//! Remote thin-client launcher over SSH command stdio.

use super::shell_quote;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use interprocess::local_socket::traits::Listener as _;
#[cfg(windows)]
use interprocess::local_socket::traits::Stream as _;
use interprocess::local_socket::ListenerNonblockingMode;
use interprocess::TryClone as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const BRIDGE_ACCEPT_POLL: Duration = Duration::from_millis(50);
#[cfg(windows)]
const BRIDGE_IO_POLL: Duration = Duration::from_millis(1);
const BRIDGE_SOCKET_PERMISSION_MODE: u32 = 0o600;
const REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_SERVER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CURRENT_PROTOCOL: u32 = crate::protocol::PROTOCOL_VERSION;
const REMOTE_BINARY_ENV_VAR: &str = "HERDR_REMOTE_BINARY";
const SSH_CONTROL_SOCKET_NAME: &str = "ctl";
const WINDOWS_POWERSHELL_EXECUTABLE: &str = "powershell.exe";
pub(crate) const REATTACH_COMMAND_ENV_VAR: &str = "HERDR_REATTACH_COMMAND";

pub(crate) const REMOTE_KEYBINDINGS_ENV_VAR: &str = "HERDR_REMOTE_KEYBINDINGS";

fn preview_update_manifest_url() -> &'static str {
    crate::distribution::PREVIEW_MANIFEST_URL
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteKeybindings {
    Local,
    Server,
}

impl RemoteKeybindings {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "local" => Ok(Self::Local),
            "server" => Ok(Self::Server),
            _ => Err("--remote-keybindings must be 'local' or 'server'".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Server => "server",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteLaunch {
    pub(crate) target: String,
    pub(crate) keybindings: RemoteKeybindings,
    pub(crate) live_handoff: bool,
    pub(crate) provision: bool,
    pub(crate) yes: bool,
    pub(crate) json: bool,
}

pub(crate) fn extract_remote_args(
    args: &[String],
) -> Result<(Vec<String>, Option<RemoteLaunch>), String> {
    let remote_requested = args
        .iter()
        .skip(1)
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--remote" || arg.starts_with("--remote="));
    let mut cleaned = Vec::with_capacity(args.len());
    if let Some(program) = args.first() {
        cleaned.push(program.clone());
    }

    let mut remote_target = None;
    let mut keybindings = RemoteKeybindings::Local;
    let mut keybindings_seen = false;
    let mut live_handoff = false;
    let mut provision = false;
    let mut yes = false;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            cleaned.extend_from_slice(&args[index..]);
            break;
        }
        if arg == "--handoff" {
            live_handoff = true;
            index += 1;
            continue;
        }
        if remote_requested && arg == "--provision" {
            provision = true;
            index += 1;
            continue;
        }
        if remote_requested && matches!(arg.as_str(), "--yes" | "-y") {
            yes = true;
            index += 1;
            continue;
        }
        if remote_requested && arg == "--json" {
            json = true;
            index += 1;
            continue;
        }
        if arg == "--remote" {
            if remote_target.is_some() {
                return Err("--remote can only be specified once".to_string());
            }
            let Some(value) = args.get(index + 1) else {
                return Err("missing value for --remote".to_string());
            };
            remote_target = Some(validate_remote_target(value)?.to_owned());
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--remote=") {
            if remote_target.is_some() {
                return Err("--remote can only be specified once".to_string());
            }
            remote_target = Some(validate_remote_target(value)?.to_owned());
            index += 1;
            continue;
        }
        if arg == "--remote-keybindings" {
            if keybindings_seen {
                return Err("--remote-keybindings can only be specified once".to_string());
            }
            let Some(value) = args.get(index + 1) else {
                return Err("missing value for --remote-keybindings".to_string());
            };
            keybindings = RemoteKeybindings::parse(value)?;
            keybindings_seen = true;
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--remote-keybindings=") {
            if keybindings_seen {
                return Err("--remote-keybindings can only be specified once".to_string());
            }
            keybindings = RemoteKeybindings::parse(value)?;
            keybindings_seen = true;
            index += 1;
            continue;
        }

        cleaned.push(arg.clone());
        index += 1;
    }

    let remote = remote_target.map(|target| RemoteLaunch {
        target,
        keybindings,
        live_handoff,
        provision,
        yes,
        json,
    });
    if remote.is_none() && keybindings_seen {
        return Err("--remote-keybindings requires --remote".to_string());
    }
    if remote.is_none() && live_handoff {
        cleaned.push("--handoff".to_string());
    }
    if !provision && (yes || json) {
        return Err("--yes and --json require --remote with --provision".to_string());
    }
    if provision && live_handoff {
        return Err("--provision cannot be combined with --handoff".to_string());
    }

    Ok((cleaned, remote))
}

fn validate_remote_target(target: &str) -> Result<&str, String> {
    if target.is_empty() {
        return Err("missing value for --remote".to_string());
    }
    if target.starts_with('-') {
        return Err("--remote target must not start with '-'".to_string());
    }
    Ok(target)
}

pub(crate) fn run_remote(remote: RemoteLaunch) -> io::Result<()> {
    let session_name = crate::session::active_name()
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_string());
    let local_socket = local_forward_socket_path(&remote.target, &session_name);
    let program = std::env::args()
        .next()
        .unwrap_or_else(|| "herdr".to_string());
    let reattach_command = reattach_command(
        &program,
        &remote.target,
        &session_name,
        remote.keybindings,
        remote.live_handoff,
    );
    let manage_ssh_config = crate::config::Config::load()
        .config
        .remote
        .manage_ssh_config;
    let interactive_progress = remote_progress_enabled(
        remote.json,
        io::stdin().is_terminal(),
        io::stderr().is_terminal(),
    );
    let remote_ssh = RemoteSsh::new(
        remote.target.clone(),
        manage_ssh_config,
        interactive_progress,
    );
    remote_ssh.progress(format_args!(
        "Connecting to {} and checking remote Herdr...",
        remote.target
    ));
    let detected = detect_remote_host(&remote_ssh)?;
    if remote.provision {
        let result = provision_remote(&remote_ssh, detected, remote.yes)?;
        print_remote_provision_result(&result, remote.json)?;
        return Ok(());
    }
    let DetectedRemoteHost {
        host,
        warm_windows_attach,
    } = detected;
    let remote_command = match host {
        RemoteHostPlatform::Unix(platform) => {
            let prepared_remote =
                prepare_remote_herdr(&remote_ssh, platform, remote.live_handoff, false)?;
            ensure_remote_server_ready(
                &remote_ssh,
                &prepared_remote.remote_herdr,
                prepared_remote.installed_or_replaced,
                prepared_remote.stop_after_install_approved,
                remote.live_handoff,
                None,
                false,
            )?;
            remote_bridge_command(&prepared_remote.remote_herdr, &session_name, None)?
        }
        RemoteHostPlatform::Windows {
            platform,
            user_profile,
            ssh_shell,
        } => {
            if remote.live_handoff {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "live handoff is not supported for Windows remote hosts",
                ));
            }
            super::windows::validate_streaming_shell(&ssh_shell)?;
            let (prepared_remote, known_server_status) = match warm_windows_attach {
                Some((remote_herdr, server_status)) => (
                    PreparedRemoteHerdr {
                        remote_herdr,
                        installed_or_replaced: false,
                        stop_after_install_approved: false,
                    },
                    Some(server_status),
                ),
                None => (
                    prepare_remote_windows_herdr(&remote_ssh, platform, &user_profile, false)?,
                    None,
                ),
            };
            ensure_remote_server_ready(
                &remote_ssh,
                &prepared_remote.remote_herdr,
                prepared_remote.installed_or_replaced,
                prepared_remote.stop_after_install_approved,
                false,
                known_server_status,
                true,
            )?;
            remote_bridge_command(
                &prepared_remote.remote_herdr,
                &session_name,
                Some(&ssh_shell),
            )?
        }
    };

    remote_ssh.progress(format_args!(
        "Opening the remote session on {}; starting its Herdr server if needed...",
        remote.target
    ));
    let _bridge = SshStdioBridge::start(
        remote.target,
        remote_command,
        local_socket.clone(),
        remote_ssh.options(),
    )?;

    run_client_process(&local_socket, &reattach_command, remote.keybindings)
}

fn remote_progress_enabled(json: bool, stdin_terminal: bool, stderr_terminal: bool) -> bool {
    !json && stdin_terminal && stderr_terminal
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemotePlatform {
    os: &'static str,
    arch: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteHostPlatform {
    Unix(RemotePlatform),
    Windows {
        platform: RemotePlatform,
        user_profile: String,
        ssh_shell: super::windows::WindowsSshShell,
    },
}

struct DetectedRemoteHost {
    host: RemoteHostPlatform,
    warm_windows_attach: Option<(RemoteHerdr, RemoteServerStatus)>,
}

impl RemotePlatform {
    fn from_uname(os: &str, arch: &str) -> Option<Self> {
        let os = match os.trim() {
            "Linux" => "linux",
            "Darwin" => "macos",
            _ => return None,
        };
        let arch = match arch.trim() {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            _ => return None,
        };
        Some(Self { os, arch })
    }

    fn windows(arch: &str) -> Option<Self> {
        let arch = match arch.trim().to_ascii_uppercase().as_str() {
            "AMD64" | "X86_64" => "x86_64",
            "ARM64" | "AARCH64" => "aarch64",
            _ => return None,
        };
        Some(Self {
            os: "windows",
            arch,
        })
    }

    fn local() -> Self {
        let os = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "unknown"
        };

        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "unknown"
        };

        Self { os, arch }
    }

    fn asset_key(&self) -> String {
        format!("{}-{}", self.os, self.arch)
    }
}

#[derive(Debug, Clone)]
struct RemoteHerdr {
    install_suffix: String,
    shell_path: String,
    platform: RemotePlatform,
    shell: RemoteShell,
    remote_sidecar: bool,
    payload_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteShell {
    Posix,
    WindowsPowerShell,
}

impl RemoteHerdr {
    fn for_platform(platform: RemotePlatform) -> Self {
        let install_suffix = ".local/bin/herdr".to_string();
        let shell_path = format!("\"$HOME/{install_suffix}\"");
        Self {
            install_suffix,
            shell_path,
            platform,
            shell: RemoteShell::Posix,
            remote_sidecar: false,
            payload_sha256: None,
        }
    }

    fn for_windows(
        platform: RemotePlatform,
        user_profile: &str,
        payload_sha256: Option<String>,
    ) -> Self {
        let install_suffix = ".herdr\\remote\\herdr.exe".to_string();
        let shell_path = format!(
            "{}\\{}",
            user_profile.trim_end_matches(['\\', '/']),
            install_suffix
        );
        Self {
            install_suffix,
            shell_path,
            platform,
            shell: RemoteShell::WindowsPowerShell,
            remote_sidecar: true,
            payload_sha256,
        }
    }

    fn with_shell_path(mut self, shell_path: String) -> Self {
        self.shell_path = shell_path;
        self
    }

    fn into_path_candidate(mut self) -> Self {
        self.remote_sidecar = false;
        self.payload_sha256 = None;
        self
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RemoteAssetRef {
    Url(String),
    Object {
        url: String,
        sha256: Option<String>,
        format: Option<String>,
    },
}

impl RemoteAssetRef {
    fn url(&self) -> &str {
        match self {
            Self::Url(url) => url,
            Self::Object { url, .. } => url,
        }
    }

    fn sha256(&self) -> Option<&str> {
        match self {
            Self::Url(_) => None,
            Self::Object { sha256, .. } => {
                sha256.as_deref().filter(|value| !value.trim().is_empty())
            }
        }
    }

    fn format(&self) -> Option<&str> {
        match self {
            Self::Url(_) => None,
            Self::Object { format, .. } => {
                format.as_deref().filter(|value| !value.trim().is_empty())
            }
        }
    }
}

#[derive(Deserialize)]
struct RemoteUpdateManifest {
    version: String,
    protocol: Option<u32>,
    assets: BTreeMap<String, RemoteAssetRef>,
    #[serde(default)]
    sha256: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_remote_manifest_releases")]
    releases: BTreeMap<String, RemoteReleaseMetadata>,
}

#[derive(Deserialize)]
struct RemoteReleaseMetadata {
    protocol: Option<u32>,
    #[serde(default)]
    assets: BTreeMap<String, RemoteAssetRef>,
    #[serde(default)]
    sha256: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RemotePreviewManifest {
    prerelease: bool,
    build_id: String,
    protocol: u32,
    assets: BTreeMap<String, RemoteAssetRef>,
    #[serde(default)]
    builds: BTreeMap<String, RemotePreviewBuildMetadata>,
}

#[derive(Deserialize)]
struct RemotePreviewBuildMetadata {
    protocol: u32,
    assets: BTreeMap<String, RemoteAssetRef>,
}

fn deserialize_remote_manifest_releases<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, RemoteReleaseMetadata>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Object(object)) => object
            .into_iter()
            .filter_map(|(version, release)| {
                serde_json::from_value::<RemoteReleaseMetadata>(release)
                    .ok()
                    .map(|metadata| (version, metadata))
            })
            .collect(),
        _ => BTreeMap::new(),
    })
}

impl RemoteUpdateManifest {
    fn release_for_version(&self, version: &str) -> Option<RemoteManifestReleaseRef<'_>> {
        if self.version.trim_start_matches('v') == version {
            return Some(RemoteManifestReleaseRef {
                protocol: self.protocol,
                assets: &self.assets,
                sha256: &self.sha256,
            });
        }

        self.releases.get(version).and_then(|release| {
            (!release.assets.is_empty()).then_some(RemoteManifestReleaseRef {
                protocol: release.protocol,
                assets: &release.assets,
                sha256: &release.sha256,
            })
        })
    }
}

#[derive(Clone, Copy)]
struct RemoteManifestReleaseRef<'a> {
    protocol: Option<u32>,
    assets: &'a BTreeMap<String, RemoteAssetRef>,
    sha256: &'a BTreeMap<String, String>,
}

fn current_version() -> String {
    crate::build_info::version()
}

fn current_channel() -> &'static str {
    crate::build_info::channel()
}

struct InstallSource {
    path: PathBuf,
    temporary_dir: Option<PathBuf>,
    kind: InstallSourceKind,
    sha256: Option<String>,
    executable_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallSourceKind {
    Executable,
    WindowsZip,
}

struct RemoteReleaseAsset {
    url: String,
    sha256: Option<String>,
    format: Option<String>,
}

struct TemporaryWindowsScript {
    path: PathBuf,
}

impl TemporaryWindowsScript {
    fn create(name: &str, script: &str) -> io::Result<Self> {
        let temporary = Self {
            path: std::env::temp_dir().join(name),
        };
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary.path)?;
        file.write_all(&super::windows::powershell_script_file_bytes(script))?;
        file.flush()?;
        Ok(temporary)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryWindowsScript {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path) {
            tracing::debug!(path = %self.path.display(), %err, "could not remove temporary Windows remote script");
        }
    }
}

struct PreparedRemoteHerdr {
    remote_herdr: RemoteHerdr,
    installed_or_replaced: bool,
    stop_after_install_approved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RemoteBinaryOutcome {
    AlreadyMatching,
    Installed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RemoteServerOutcome {
    Started,
    Reloaded,
    Restarted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteProvisionServerAction {
    Start,
    Reload,
    Restart,
}

#[derive(Debug, Serialize)]
struct RemoteProvisionResult {
    target: String,
    platform: String,
    binary: String,
    binary_outcome: RemoteBinaryOutcome,
    server_outcome: RemoteServerOutcome,
    version: String,
    protocol: u32,
}

#[derive(Clone)]
struct ManagedSshOptions {
    config_path: PathBuf,
    control_path: Option<PathBuf>,
}

struct ManagedSshConfig {
    options: ManagedSshOptions,
}

impl Drop for ManagedSshConfig {
    fn drop(&mut self) {
        if let Some(dir) = self.options.config_path.parent() {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

struct RemoteSsh {
    target: String,
    managed_config: Option<ManagedSshConfig>,
    interactive_progress: bool,
}

impl RemoteSsh {
    fn new(target: String, manage_ssh_config: bool, interactive_progress: bool) -> Self {
        let managed_config = if manage_ssh_config {
            write_managed_ssh_config()
                .inspect_err(|err| {
                    tracing::debug!(%err, "could not write managed ssh config; using plain ssh");
                })
                .ok()
        } else {
            None
        };

        Self {
            target,
            managed_config,
            interactive_progress,
        }
    }

    fn target(&self) -> &str {
        &self.target
    }

    fn progress(&self, message: impl std::fmt::Display) {
        if self.interactive_progress {
            eprintln!("{message}");
        }
    }

    fn options(&self) -> Option<&ManagedSshOptions> {
        self.managed_config.as_ref().map(|config| &config.options)
    }

    fn command(&self) -> Command {
        let mut command = self.base_command();
        command.arg("-T").arg(&self.target);
        command
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new("ssh");
        apply_managed_ssh_options(&mut command, self.options());
        command
    }

    fn scp_command(&self) -> Command {
        let mut command = Command::new("scp");
        command.arg("-O");
        apply_managed_scp_options(&mut command, self.options());
        command
    }

    fn sh_output(&self, script: &str) -> io::Result<Output> {
        let mut child = self
            .command()
            .arg("/bin/sh -s")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let write_result = if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(script.as_bytes())
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh bootstrap stdin missing",
            ))
        };
        let output = child.wait_with_output()?;
        write_result?;
        Ok(output)
    }

    fn user_shell_output(&self, command: &str) -> io::Result<Output> {
        self.command().arg(command).output()
    }

    fn powershell_output(&self, script: &str) -> io::Result<Output> {
        self.user_shell_output(&super::windows::powershell_script_command(script))
    }

    fn powershell_command_output(&self, command: &str) -> io::Result<Output> {
        self.user_shell_output(command)
    }

    fn powershell_script_output(
        &self,
        remote_herdr: &RemoteHerdr,
        script: &str,
    ) -> io::Result<Output> {
        let script_name = windows_bootstrap_script_name()?;
        let temporary = TemporaryWindowsScript::create(&script_name, script)?;
        let destination = windows_scp_destination(&self.target, &script_name);
        let transfer = self
            .scp_command()
            .arg(temporary.path())
            .arg(&destination)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to start Windows remote bootstrap transfer: {err}"),
                )
            })?;
        drop(temporary);
        if !transfer.status.success() {
            return Err(command_failed(
                "Windows remote bootstrap transfer failed",
                &transfer,
            ));
        }

        let remote_path = windows_remote_home_path(remote_herdr, &script_name)?;
        self.powershell_command_output(&super::windows::powershell_script_file_command(
            &remote_path,
        ))
    }

    fn windows_herdr_output(
        &self,
        remote_herdr: &RemoteHerdr,
        arguments: &[String],
    ) -> io::Result<Output> {
        let executable = (remote_herdr.shell == RemoteShell::WindowsPowerShell)
            .then_some(remote_herdr.shell_path.as_str());
        self.powershell_command_output(&super::windows::powershell_herdr_command(
            executable,
            arguments,
            remote_herdr.remote_sidecar,
        ))
    }

    fn install_herdr(&self, remote_herdr: &RemoteHerdr, source_path: &Path) -> io::Result<()> {
        let output = self.sh_output(&remote_install_prepare_script(remote_herdr))?;
        if !output.status.success() {
            return Err(command_failed("remote install preparation failed", &output));
        }
        let (tmp_path, dest_path) = parse_remote_install_paths(&output.stdout)?;

        let mut child = self
            .command()
            .arg(remote_install_stream_command(&tmp_path))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                io::Error::new(err.kind(), format!("failed to start ssh install: {err}"))
            })?;

        let mut source = File::open(source_path)?;
        let copy_result = if let Some(mut stdin) = child.stdin.take() {
            io::copy(&mut source, &mut stdin).map(|_| ())
        } else {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh install stdin missing",
            ))
        };
        let status = child.wait()?;
        copy_result?;

        if status.success() {
            let output = self.sh_output(&remote_install_commit_script(&tmp_path, &dest_path))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(command_failed("remote install commit failed", &output))
            }
        } else {
            Err(io::Error::other(format!(
                "remote install exited with {status}"
            )))
        }
    }

    fn stage_windows_payload(
        &self,
        remote_herdr: &RemoteHerdr,
        source_path: &Path,
        expected_sha256: &str,
    ) -> io::Result<String> {
        let archive_name = windows_payload_archive_name()?;
        let temporary_archive = windows_payload_archive_path(remote_herdr, &archive_name)?;
        self.progress(format_args!(
            "Preparing a temporary Herdr install on {}...",
            self.target
        ));
        let output = self.powershell_script_output(
            remote_herdr,
            &windows_install_prepare_script(remote_herdr, &archive_name),
        )?;
        if !output.status.success() {
            return Err(command_failed(
                "Windows remote install preparation failed",
                &output,
            ));
        }

        let destination = windows_scp_destination(&self.target, &format!(".herdr/{archive_name}"));
        self.progress(format_args!(
            "Transferring Herdr {} to {}...",
            current_version(),
            self.target
        ));
        let transfer = self
            .scp_command()
            .arg(source_path)
            .arg(&destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|err| {
                io::Error::new(
                    err.kind(),
                    format!("failed to start Windows remote ZIP transfer: {err}"),
                )
            })?;
        if !transfer.success() {
            let _ = self.powershell_script_output(
                remote_herdr,
                &windows_install_cleanup_archive_script(&temporary_archive),
            );
            return Err(io::Error::other(format!(
                "Windows remote ZIP transfer to {destination} exited with {transfer}"
            )));
        }

        self.progress(format_args!(
            "Validating the Herdr package on {}...",
            self.target
        ));
        let output = self.powershell_script_output(
            remote_herdr,
            &windows_install_stage_script(remote_herdr, &temporary_archive, expected_sha256),
        )?;
        if !output.status.success() {
            return Err(command_failed(
                "Windows remote payload staging failed",
                &output,
            ));
        }
        let stdout = String::from_utf8(output.stdout).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows remote payload staging returned non-UTF-8 output",
            )
        })?;
        let stages = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        let [stage] = stages.as_slice() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows remote payload staging did not return exactly one stage path",
            ));
        };
        Ok((*stage).to_string())
    }

    fn activate_windows_payload(&self, remote_herdr: &RemoteHerdr, stage: &str) -> io::Result<()> {
        let output = self.powershell_script_output(
            remote_herdr,
            &windows_install_activate_script(remote_herdr, stage),
        )?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failed(
                "Windows remote payload activation failed",
                &output,
            ))
        }
    }

    fn cleanup_windows_stage(&self, remote_herdr: &RemoteHerdr, stage: &str) {
        let _ = self.powershell_script_output(
            remote_herdr,
            &windows_install_cleanup_stage_script(remote_herdr, stage),
        );
    }
}

fn remote_install_prepare_script(remote_herdr: &RemoteHerdr) -> String {
    format!(
        r#"set -eu
dest="$HOME/{install_suffix}"
dir="${{dest%/*}}"
mkdir -p "$dir"
tmp="${{dest}}.tmp.$$"
printf '%s\0%s\0' "$tmp" "$dest"
"#,
        install_suffix = remote_herdr.install_suffix
    )
}

fn parse_remote_install_paths(stdout: &[u8]) -> io::Result<(String, String)> {
    let mut parts = stdout.split(|byte| *byte == 0);
    let tmp_path = parts.next().unwrap_or_default();
    let dest_path = parts.next().unwrap_or_default();
    if tmp_path.is_empty() || dest_path.is_empty() {
        return Err(io::Error::other(
            "remote install preparation did not return destination paths",
        ));
    }
    let tmp_path = String::from_utf8(tmp_path.to_vec()).map_err(|err| {
        io::Error::other(format!(
            "remote install temporary path is not valid UTF-8: {err}"
        ))
    })?;
    let dest_path = String::from_utf8(dest_path.to_vec()).map_err(|err| {
        io::Error::other(format!(
            "remote install destination path is not valid UTF-8: {err}"
        ))
    })?;
    Ok((tmp_path, dest_path))
}

fn remote_install_stream_command(tmp_path: &str) -> String {
    format!("tee {}", shell_quote(tmp_path))
}

fn remote_install_commit_script(tmp_path: &str, dest_path: &str) -> String {
    format!(
        "set -eu\nchmod 755 {tmp_path}\nmv {tmp_path} {dest_path}\n",
        tmp_path = shell_quote(tmp_path),
        dest_path = shell_quote(dest_path)
    )
}

fn windows_sidecar_root(remote_herdr: &RemoteHerdr) -> String {
    remote_herdr
        .shell_path
        .strip_suffix("\\herdr.exe")
        .unwrap_or(&remote_herdr.shell_path)
        .to_string()
}

fn windows_payload_archive_name() -> io::Result<String> {
    Ok(format!("payload-{}.zip", windows_transfer_id()?))
}

fn windows_bootstrap_script_name() -> io::Result<String> {
    Ok(format!("bootstrap-{}.ps1", windows_transfer_id()?))
}

fn windows_transfer_id() -> io::Result<String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| io::Error::other(format!("system clock precedes Unix epoch: {err}")))?
        .as_nanos();
    Ok(format!("{}-{timestamp:x}", std::process::id()))
}

fn windows_payload_archive_path(
    remote_herdr: &RemoteHerdr,
    archive_name: &str,
) -> io::Result<String> {
    let root = windows_sidecar_root(remote_herdr);
    let (parent, _) = root.rsplit_once('\\').ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Windows remote sidecar root has no parent: {root}"),
        )
    })?;
    Ok(format!("{parent}\\{archive_name}"))
}

fn windows_remote_home_path(remote_herdr: &RemoteHerdr, name: &str) -> io::Result<String> {
    let suffix = format!("\\{}", remote_herdr.install_suffix);
    let home = remote_herdr
        .shell_path
        .strip_suffix(&suffix)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Windows remote executable is outside its detected user profile: {}",
                    remote_herdr.shell_path
                ),
            )
        })?;
    Ok(format!("{home}\\{name}"))
}

fn windows_scp_destination(target: &str, relative_path: &str) -> String {
    if let Some(authority) = target.strip_prefix("ssh://") {
        format!("scp://{}/{relative_path}", authority.trim_end_matches('/'))
    } else {
        format!("{target}:{relative_path}")
    }
}

fn windows_install_prepare_script(remote_herdr: &RemoteHerdr, archive_name: &str) -> String {
    let root = windows_sidecar_root(remote_herdr);
    super::windows::powershell_bootstrap_script(&format!(
        "Invoke-HerdrRemotePrepareInstall -Destination {} -ArchiveName {}",
        super::windows::powershell_quote(&root),
        super::windows::powershell_quote(archive_name),
    ))
}

fn windows_install_cleanup_archive_script(archive_path: &str) -> String {
    super::windows::powershell_bootstrap_script(&format!(
        "Remove-HerdrRemoteArchive -Archive {}",
        super::windows::powershell_quote(archive_path),
    ))
}

fn windows_install_stage_script(
    remote_herdr: &RemoteHerdr,
    archive_path: &str,
    expected_sha256: &str,
) -> String {
    let root = windows_sidecar_root(remote_herdr);
    let session = crate::session::active_name()
        .filter(|name| name != crate::session::DEFAULT_SESSION_NAME)
        .unwrap_or_default();
    super::windows::powershell_bootstrap_script(&format!(
        "Invoke-HerdrRemoteStageInstall -Archive {} -Destination {} -ExpectedSha256 {} -ExpectedRuntimeVersion {} -ExpectedProtocol {} -SessionName {}",
        super::windows::powershell_quote(archive_path),
        super::windows::powershell_quote(&root),
        super::windows::powershell_quote(expected_sha256),
        super::windows::powershell_quote(&current_version()),
        CURRENT_PROTOCOL,
        super::windows::powershell_quote(&session),
    ))
}

fn windows_install_activate_script(remote_herdr: &RemoteHerdr, stage: &str) -> String {
    let root = windows_sidecar_root(remote_herdr);
    super::windows::powershell_bootstrap_script(&format!(
        "Invoke-HerdrRemoteActivateInstall -Stage {} -Destination {}",
        super::windows::powershell_quote(stage),
        super::windows::powershell_quote(&root),
    ))
}

fn windows_install_cleanup_stage_script(remote_herdr: &RemoteHerdr, stage: &str) -> String {
    let root = windows_sidecar_root(remote_herdr);
    super::windows::powershell_bootstrap_script(&format!(
        "Remove-HerdrRemoteStage -Stage {} -Destination {}",
        super::windows::powershell_quote(stage),
        super::windows::powershell_quote(&root),
    ))
}

impl Drop for RemoteSsh {
    fn drop(&mut self) {
        let Some(_options) = self
            .managed_config
            .as_ref()
            .map(|config| &config.options)
            .filter(|options| options.control_path.is_some())
        else {
            return;
        };

        let _ = self
            .base_command()
            .arg("-O")
            .arg("exit")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg(&self.target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn apply_managed_ssh_options(command: &mut Command, options: Option<&ManagedSshOptions>) {
    let Some(options) = options else {
        return;
    };

    command.arg("-F").arg(&options.config_path);
    if let Some(control_path) = &options.control_path {
        command
            .arg("-S")
            .arg(control_path)
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=yes");
    }
}

fn apply_managed_scp_options(command: &mut Command, options: Option<&ManagedSshOptions>) {
    let Some(options) = options else {
        return;
    };

    command.arg("-F").arg(&options.config_path);
    if let Some(control_path) = &options.control_path {
        command
            .arg("-o")
            .arg(format!("ControlPath={}", control_path.display()))
            .arg("-o")
            .arg("ControlMaster=auto")
            .arg("-o")
            .arg("ControlPersist=yes");
    }
}

impl InstallSource {
    fn persistent(path: PathBuf) -> Self {
        Self {
            path,
            temporary_dir: None,
            kind: InstallSourceKind::Executable,
            sha256: None,
            executable_sha256: None,
        }
    }

    fn temporary(path: PathBuf, temporary_dir: PathBuf) -> Self {
        Self {
            path,
            temporary_dir: Some(temporary_dir),
            kind: InstallSourceKind::Executable,
            sha256: None,
            executable_sha256: None,
        }
    }

    fn windows_zip(path: PathBuf, temporary_dir: Option<PathBuf>, sha256: String) -> Self {
        Self {
            path,
            temporary_dir,
            kind: InstallSourceKind::WindowsZip,
            sha256: Some(sha256),
            executable_sha256: None,
        }
    }

    fn local_windows_zip(
        path: PathBuf,
        temporary_dir: PathBuf,
        sha256: String,
        executable_sha256: String,
    ) -> Self {
        Self {
            path,
            temporary_dir: Some(temporary_dir),
            kind: InstallSourceKind::WindowsZip,
            sha256: Some(sha256),
            executable_sha256: Some(executable_sha256),
        }
    }

    fn cleanup(&self) {
        if let Some(dir) = &self.temporary_dir {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

fn prepare_remote_herdr(
    ssh: &RemoteSsh,
    platform: RemotePlatform,
    live_handoff_enabled: bool,
    yes: bool,
) -> io::Result<PreparedRemoteHerdr> {
    let remote_herdr = RemoteHerdr::for_platform(platform);
    let override_binary = remote_binary_override_path()?;
    let remote_binary_candidates = remote_binary_candidates(ssh, &remote_herdr)?;

    if override_binary.is_none() {
        for candidate in &remote_binary_candidates {
            if remote_binary_matches(ssh, candidate).unwrap_or(false) {
                return Ok(PreparedRemoteHerdr {
                    remote_herdr: candidate.clone(),
                    installed_or_replaced: false,
                    stop_after_install_approved: false,
                });
            }
        }
        if remote_binary_matches(ssh, &remote_herdr)? {
            return Ok(PreparedRemoteHerdr {
                remote_herdr,
                installed_or_replaced: false,
                stop_after_install_approved: false,
            });
        }
    }

    let mut stop_after_install_approved = false;
    if let Some(status_probe_herdr) = remote_binary_candidates.first().or_else(|| {
        remote_binary_exists(ssh, &remote_herdr)
            .ok()
            .and_then(|exists| exists.then_some(&remote_herdr))
    }) {
        stop_after_install_approved = if yes {
            matches!(
                remote_server_status(ssh, status_probe_herdr)?,
                RemoteServerStatus::Running { .. }
            )
        } else {
            confirm_remote_install_with_running_server(
                ssh,
                status_probe_herdr,
                live_handoff_enabled,
            )?
        };
    }
    confirm_remote_install(
        ssh.target(),
        &remote_herdr,
        &install_source_description(&remote_herdr.platform, override_binary.as_deref()),
        yes || stop_after_install_approved,
    )?;
    ssh.progress(format_args!(
        "Preparing Herdr {} for {}...",
        current_version(),
        ssh.target()
    ));
    let source = resolve_install_source(&remote_herdr.platform, override_binary)?;
    ssh.progress(format_args!(
        "Transferring and installing Herdr {} on {}...",
        current_version(),
        ssh.target()
    ));
    let install_result = ssh.install_herdr(&remote_herdr, &source.path);
    source.cleanup();
    install_result?;

    ssh.progress(format_args!(
        "Verifying Herdr {} on {}...",
        current_version(),
        ssh.target()
    ));
    if !remote_binary_matches(ssh, &remote_herdr)? {
        return Err(io::Error::other(format!(
            "installed remote herdr at {}, but it did not report version {}",
            remote_herdr.shell_path,
            current_version()
        )));
    }
    warn_if_remote_bin_not_on_path(ssh)?;
    ssh.progress(format_args!(
        "Herdr {} is installed and verified on {}.",
        current_version(),
        ssh.target()
    ));

    Ok(PreparedRemoteHerdr {
        remote_herdr,
        installed_or_replaced: true,
        stop_after_install_approved,
    })
}

fn prepare_remote_windows_herdr(
    ssh: &RemoteSsh,
    platform: RemotePlatform,
    user_profile: &str,
    yes: bool,
) -> io::Result<PreparedRemoteHerdr> {
    let override_payload = remote_binary_override_path()?;
    let (expected_executable_sha256, allow_path_candidate) = if override_payload.is_some() {
        (None, false)
    } else {
        local_windows_attach_identity()?
    };
    let managed =
        RemoteHerdr::for_windows(platform.clone(), user_profile, expected_executable_sha256);
    let path_candidate = if allow_path_candidate {
        windows_remote_binary_on_path(ssh, &managed)?
    } else {
        None
    };
    if let Some(candidate) = path_candidate.as_ref() {
        if remote_binary_matches(ssh, candidate)? {
            return Ok(PreparedRemoteHerdr {
                remote_herdr: candidate.clone(),
                installed_or_replaced: false,
                stop_after_install_approved: false,
            });
        }
    }
    if remote_binary_matches(ssh, &managed)? {
        return Ok(PreparedRemoteHerdr {
            remote_herdr: managed,
            installed_or_replaced: false,
            stop_after_install_approved: false,
        });
    }

    let status_probe = match path_candidate.as_ref() {
        Some(candidate) => Some(candidate),
        None if remote_binary_exists(ssh, &managed)? => Some(&managed),
        None => None,
    };
    let stop_before_activation = match status_probe {
        Some(remote) if yes => matches!(
            remote_server_status(ssh, remote)?,
            RemoteServerStatus::Running { .. }
        ),
        Some(remote) => confirm_remote_install_with_running_server(ssh, remote, false)?,
        None => false,
    };
    confirm_remote_install(
        ssh.target(),
        &managed,
        &install_source_description(&managed.platform, override_payload.as_deref()),
        yes || stop_before_activation,
    )?;

    ssh.progress(format_args!(
        "Preparing Herdr {} for {}...",
        current_version(),
        ssh.target()
    ));
    let source = resolve_windows_install_source(&platform, override_payload)?;
    if source.kind != InstallSourceKind::WindowsZip {
        source.cleanup();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows remote installation source is not a portable ZIP",
        ));
    }
    let expected_sha256 = source.sha256.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows remote portable payload is missing its verified SHA-256 identity",
        )
    })?;
    if let Some(expected_executable_sha256) = managed.payload_sha256.as_deref() {
        if source.executable_sha256.as_deref() != Some(expected_executable_sha256) {
            source.cleanup();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "local Windows portable payload executable identity does not match the running client",
            ));
        }
    }
    let stage_result = ssh.stage_windows_payload(&managed, &source.path, expected_sha256);
    source.cleanup();
    let stage = stage_result?;

    if stop_before_activation {
        let remote = status_probe.ok_or_else(|| {
            io::Error::other("remote server stop was approved without a status binary")
        })?;
        if let Err(err) = stop_remote_server(ssh, remote) {
            ssh.cleanup_windows_stage(&managed, &stage);
            return Err(err);
        }
    }
    ssh.progress(format_args!(
        "Activating Herdr {} on {}...",
        current_version(),
        ssh.target()
    ));
    if let Err(err) = ssh.activate_windows_payload(&managed, &stage) {
        ssh.cleanup_windows_stage(&managed, &stage);
        return Err(err);
    }
    ssh.progress(format_args!(
        "Verifying Herdr {} on {}...",
        current_version(),
        ssh.target()
    ));
    if !remote_binary_matches(ssh, &managed)? {
        return Err(io::Error::other(format!(
            "installed Windows remote Herdr at {}, but its binary, version, protocol, or payload did not match",
            managed.shell_path
        )));
    }
    ssh.progress(format_args!(
        "Herdr {} is installed and verified on {}.",
        current_version(),
        ssh.target()
    ));

    Ok(PreparedRemoteHerdr {
        remote_herdr: managed,
        installed_or_replaced: true,
        stop_after_install_approved: stop_before_activation,
    })
}

fn windows_remote_binary_on_path(
    ssh: &RemoteSsh,
    managed: &RemoteHerdr,
) -> io::Result<Option<RemoteHerdr>> {
    let script = "$command = Get-Command -Name 'herdr.exe' -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1; if ($null -eq $command) { exit 1 }; [Console]::Out.WriteLine([string]$command.Source)";
    let output = ssh.powershell_output(script)?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !valid_windows_user_profile(&path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows PATH candidate is not an absolute drive path",
        ));
    }
    Ok(Some(
        managed.clone().with_shell_path(path).into_path_candidate(),
    ))
}

fn detect_remote_platform(ssh: &RemoteSsh) -> io::Result<RemotePlatform> {
    let output = ssh.sh_output("uname -s\nuname -m\n")?;
    if !output.status.success() {
        return Err(command_failed("remote platform detection failed", &output));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let os = lines.next().unwrap_or_default();
    let arch = lines.next().unwrap_or_default();
    RemotePlatform::from_uname(os, arch).ok_or_else(|| {
        io::Error::other(format!(
            "unsupported remote platform: {} {}",
            os.trim(),
            arch.trim()
        ))
    })
}

fn detect_remote_host(ssh: &RemoteSsh) -> io::Result<DetectedRemoteHost> {
    if let Some(detected) = detect_remote_windows_attach(ssh)? {
        return Ok(detected);
    }
    detect_remote_platform(ssh).map(|platform| DetectedRemoteHost {
        host: RemoteHostPlatform::Unix(platform),
        warm_windows_attach: None,
    })
}

fn detect_remote_windows_attach(ssh: &RemoteSsh) -> io::Result<Option<DetectedRemoteHost>> {
    let (expected_payload_sha256, allow_path_candidate) = local_windows_attach_identity()?;
    let session_name =
        crate::session::active_name().filter(|name| name != crate::session::DEFAULT_SESSION_NAME);
    let command = super::windows::powershell_attach_probe_command(
        &current_version(),
        CURRENT_PROTOCOL,
        expected_payload_sha256.as_deref(),
        allow_path_candidate,
        session_name.as_deref(),
    );
    let output = ssh.powershell_command_output(&command)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if remote_command_missing(&stderr, WINDOWS_POWERSHELL_EXECUTABLE) {
            return Ok(None);
        }
        return Err(command_failed(
            "Windows remote attach probe failed",
            &output,
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let probe: WindowsAttachProbeJson = serde_json::from_str(stdout.trim()).map_err(|err| {
        io::Error::other(format!(
            "could not parse Windows remote attach probe JSON from `{}`: {err}",
            stdout.trim()
        ))
    })?;
    if !probe.os.eq_ignore_ascii_case("Windows_NT") {
        return Err(io::Error::other(format!(
            "Windows remote attach probe reported unsupported OS {}",
            probe.os
        )));
    }
    let platform = RemotePlatform::windows(&probe.arch).ok_or_else(|| {
        io::Error::other(format!(
            "unsupported Windows remote architecture: {}",
            probe.arch
        ))
    })?;
    if !valid_windows_user_profile(&probe.user_profile) {
        return Err(io::Error::other(format!(
            "Windows remote attach probe reported invalid user profile {}",
            probe.user_profile
        )));
    }

    let mut warm_windows_attach = None;
    if let Some(selected) = probe.selected {
        if selected.client.version != current_version()
            || selected.client.protocol != CURRENT_PROTOCOL
        {
            return Err(io::Error::other(
                "Windows remote attach probe selected a mismatched Herdr binary",
            ));
        }
        let managed = RemoteHerdr::for_windows(
            platform.clone(),
            &probe.user_profile,
            expected_payload_sha256,
        );
        let remote_herdr = if selected.sidecar {
            if !remote_binary_paths_match(&selected.path, &managed.shell_path) {
                return Err(io::Error::other(
                    "Windows remote attach probe reported an unexpected sidecar path",
                ));
            }
            managed
        } else {
            if !allow_path_candidate {
                return Err(io::Error::other(
                    "Windows remote attach probe selected PATH for a local development build",
                ));
            }
            managed.with_shell_path(selected.path).into_path_candidate()
        };
        warm_windows_attach = Some((
            remote_herdr,
            remote_server_status_from_json(selected.server),
        ));
    }

    Ok(Some(DetectedRemoteHost {
        host: RemoteHostPlatform::Windows {
            platform,
            user_profile: probe.user_profile,
            ssh_shell: super::windows::WindowsSshShell::from_default_shell(&probe.default_shell),
        },
        warm_windows_attach,
    }))
}

#[cfg(windows)]
fn local_windows_attach_identity() -> io::Result<(Option<String>, bool)> {
    if option_env!("HERDR_RELEASE_VERSION").is_some() {
        return Ok((None, true));
    }
    let executable = std::env::current_exe()?;
    if crate::update::is_package_manager_managed_exe_path(&executable) {
        return Ok((None, true));
    }
    file_sha256(&executable).map(|sha256| (Some(sha256), false))
}

#[cfg(not(windows))]
fn local_windows_attach_identity() -> io::Result<(Option<String>, bool)> {
    Ok((None, true))
}

fn remote_command_missing(stderr: &str, executable: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains(&executable.to_ascii_lowercase())
        && (stderr.contains("not recognized")
            || stderr.contains("not found")
            || stderr.contains("no such file"))
}

fn valid_windows_user_profile(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && !path.contains(['\r', '\n', '\0'])
}

fn remote_binary_candidates(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<Vec<RemoteHerdr>> {
    let mut candidates = Vec::new();

    if let Some(path_candidate) = remote_binary_on_path_any(ssh, remote_herdr)? {
        push_if_new_remote_binary_candidate(&mut candidates, path_candidate);
    }

    let output = ssh.sh_output(&known_remote_binary_candidate_script(
        &remote_herdr.platform,
    ))?;
    if !output.status.success() {
        return Err(command_failed("remote binary discovery failed", &output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for candidate in remote_herdrs_from_path_discovery(remote_herdr, &stdout) {
        push_if_new_remote_binary_candidate(&mut candidates, candidate);
    }

    Ok(candidates)
}

fn push_if_new_remote_binary_candidate(candidates: &mut Vec<RemoteHerdr>, candidate: RemoteHerdr) {
    if !candidates
        .iter()
        .any(|existing| existing.shell_path == candidate.shell_path)
    {
        candidates.push(candidate);
    }
}

fn known_remote_binary_candidate_script(platform: &RemotePlatform) -> String {
    let mut script = String::from(
        r#"home=${HOME:-}
user=${USER:-}
version="#,
    );
    script.push_str(&shell_quote(&current_version()));
    script.push_str(
        r#"
emit() {
    path=$1
    if [ -n "$path" ] && [ -x "$path" ]; then
        printf '%s\n' "$path"
    fi
}
if [ -n "$home" ]; then
    emit "$home/.local/bin/herdr"
fi
"#,
    );
    if platform.os == "macos" {
        script.push_str(
            r#"    emit "/opt/homebrew/bin/herdr"
    emit "/usr/local/bin/herdr"
"#,
        );
    } else if platform.os == "linux" {
        script.push_str(
            r#"    emit "/home/linuxbrew/.linuxbrew/bin/herdr"
"#,
        );
    }
    script.push_str(
        r#"if [ -n "$home" ]; then
    emit "$home/.local/share/mise/installs/herdr/$version/bin/herdr"
    emit "$home/.local/share/mise/installs/herdr/$version/herdr"
    emit "$home/.local/share/mise/installs/github-ogulcancelik-herdr/$version/herdr"
    emit "$home/.nix-profile/bin/herdr"
fi
if [ -n "$user" ]; then
    emit "/etc/profiles/per-user/$user/bin/herdr"
fi
emit "/nix/var/nix/profiles/default/bin/herdr"
emit "/run/current-system/sw/bin/herdr"
"#,
    );

    script
}

fn remote_binary_on_path_any(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<Option<RemoteHerdr>> {
    let output = ssh.user_shell_output("command -v herdr")?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(candidate) = remote_herdr_from_path_discovery(remote_herdr, &stdout) {
            return Ok(Some(candidate));
        }
    }

    // Non-POSIX login shells such as xonsh reject `command -v`; retry through
    // /bin/sh while retaining the login-shell probe for shell-initialized PATHs.
    let output = ssh.sh_output("command -v herdr\n")?;
    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(remote_herdr_from_path_discovery(remote_herdr, &stdout))
}

fn remote_herdrs_from_path_discovery(remote_herdr: &RemoteHerdr, stdout: &str) -> Vec<RemoteHerdr> {
    stdout
        .lines()
        .filter_map(|path| remote_herdr_from_path(remote_herdr, path))
        .collect()
}

fn remote_herdr_from_path_discovery(
    remote_herdr: &RemoteHerdr,
    stdout: &str,
) -> Option<RemoteHerdr> {
    stdout
        .lines()
        .find_map(|path| remote_herdr_from_path(remote_herdr, path))
}

fn remote_herdr_from_path(remote_herdr: &RemoteHerdr, path: &str) -> Option<RemoteHerdr> {
    let path = path.trim();
    if !path.starts_with('/') {
        return None;
    }
    if is_mise_shim_path(path) {
        return None;
    }
    Some(remote_herdr.clone().with_shell_path(shell_quote(path)))
}

fn is_mise_shim_path(path: &str) -> bool {
    path.ends_with("/mise/shims/herdr")
}

fn remote_binary_matches(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<bool> {
    match remote_herdr.shell {
        RemoteShell::Posix => {
            let command = format!(
                "test -x {0} && {0} --version && {0} status client --json",
                remote_herdr.shell_path
            );
            let output = ssh.sh_output(&command)?;
            if !output.status.success() {
                return Ok(false);
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut lines = stdout.lines();
            let version = lines.next().unwrap_or_default().trim();
            let status = lines.next().unwrap_or_default();
            Ok(version == format!("herdr {}", current_version())
                && remote_client_status_matches_current(status))
        }
        RemoteShell::WindowsPowerShell => {
            let output =
                ssh.powershell_command_output(&super::windows::powershell_herdr_probe_command(
                    &remote_herdr.shell_path,
                    remote_herdr.remote_sidecar,
                    remote_herdr.payload_sha256.as_deref(),
                ))?;
            if !output.status.success() {
                return Ok(false);
            }
            Ok(remote_client_status_matches_current(
                &String::from_utf8_lossy(&output.stdout),
            ))
        }
    }
}

fn remote_binary_exists(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<bool> {
    match remote_herdr.shell {
        RemoteShell::Posix => {
            let command = format!("test -x {}", remote_herdr.shell_path);
            Ok(ssh.sh_output(&command)?.status.success())
        }
        RemoteShell::WindowsPowerShell => {
            let script = format!(
                "if ([IO.File]::Exists({})) {{ exit 0 }} else {{ exit 1 }}",
                super::windows::powershell_quote(&remote_herdr.shell_path)
            );
            Ok(ssh.powershell_output(&script)?.status.success())
        }
    }
}

fn remote_binary_override_path() -> io::Result<Option<PathBuf>> {
    let Some(value) = std::env::var_os(REMOTE_BINARY_ENV_VAR) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{REMOTE_BINARY_ENV_VAR} must not be empty"),
        ));
    }

    let path = PathBuf::from(value);
    let metadata = fs::metadata(&path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to inspect {REMOTE_BINARY_ENV_VAR} path {}: {err}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{REMOTE_BINARY_ENV_VAR} path is not a file: {}",
                path.display()
            ),
        ));
    }

    Ok(Some(path))
}

fn install_source_description(platform: &RemotePlatform, override_binary: Option<&Path>) -> String {
    install_source_description_for(
        platform,
        override_binary,
        local_binary_can_seed_remote(platform),
    )
}

fn install_source_description_for(
    platform: &RemotePlatform,
    override_binary: Option<&Path>,
    local_binary_can_seed_remote: bool,
) -> String {
    if let Some(path) = override_binary {
        return format!("{REMOTE_BINARY_ENV_VAR} ({})", path.display());
    }

    if local_binary_can_seed_remote {
        "the current local herdr binary".to_string()
    } else {
        format!(
            "the {} {} asset for {}",
            current_version(),
            current_channel(),
            platform.asset_key()
        )
    }
}

fn resolve_install_source(
    platform: &RemotePlatform,
    override_binary: Option<PathBuf>,
) -> io::Result<InstallSource> {
    if let Some(path) = override_binary {
        return Ok(InstallSource::persistent(path));
    }

    if *platform == RemotePlatform::local() {
        let path = std::env::current_exe()?;
        if !crate::update::is_package_manager_managed_exe_path(&path) {
            return Ok(InstallSource::persistent(path));
        }
    }

    download_release_asset(platform)
}

fn resolve_windows_install_source(
    platform: &RemotePlatform,
    override_payload: Option<PathBuf>,
) -> io::Result<InstallSource> {
    if let Some(path) = override_payload {
        validate_windows_zip_path(&path)?;
        let sha256 = file_sha256(&path)?;
        return Ok(InstallSource::windows_zip(path, None, sha256));
    }
    if local_binary_compatible_with_remote(platform) {
        let path = std::env::current_exe()?;
        if !crate::update::is_package_manager_managed_exe_path(&path) {
            return package_local_windows_runtime(&path);
        }
    }
    download_release_asset(&RemotePlatform {
        os: "windows",
        arch: "x86_64",
    })
}

fn validate_windows_zip_path(path: &Path) -> io::Result<()> {
    let mut file = File::open(path)?;
    let mut signature = [0_u8; 4];
    std::io::Read::read_exact(&mut file, &mut signature).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("Windows remote payload ZIP is unreadable: {err}"),
        )
    })?;
    if !matches!(signature, [b'P', b'K', 3, 4] | [b'P', b'K', 5, 6]) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{REMOTE_BINARY_ENV_VAR} must identify a complete Windows portable ZIP, not a loose executable"
            ),
        ));
    }
    Ok(())
}

fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

const WINDOWS_PORTABLE_RUNTIME_REQUIRED_FILES: &[&str] = &[
    "LICENSE.txt",
    "conpty/herdr-conpty.json",
    "conpty/conpty.dll",
    "conpty/x64/OpenConsole.exe",
    "conpty/arm64/OpenConsole.exe",
    "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-LICENSE.txt",
    "THIRD-PARTY-NOTICES/Microsoft.Windows.Console.ConPTY-NOTICE.md",
];

fn local_windows_runtime_root(executable: &Path) -> io::Result<&Path> {
    let root = executable.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "local Windows Herdr executable has no parent directory",
        )
    })?;
    for relative in WINDOWS_PORTABLE_RUNTIME_REQUIRED_FILES {
        let path = root.join(relative.replace('/', "\\"));
        if !path.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "local Windows Herdr runtime is missing {relative}; run from a complete portable or managed runtime, or set {REMOTE_BINARY_ENV_VAR} to a complete portable ZIP"
                ),
            ));
        }
    }
    Ok(root)
}

#[cfg(windows)]
fn run_local_windows_payload_packager(
    source_root: &Path,
    stage: &Path,
    archive: &Path,
) -> io::Result<()> {
    let job = crate::platform::ChildProcessJob::new_kill_on_close()?;
    let encoded_script = super::windows::local_payload_package_encoded_script();
    let mut child = Command::new(WINDOWS_POWERSHELL_EXECUTABLE)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded_script,
        ])
        .env("HERDR_LOCAL_PAYLOAD_SOURCE", source_root)
        .env("HERDR_LOCAL_PAYLOAD_STAGE", stage)
        .env("HERDR_LOCAL_PAYLOAD_ARCHIVE", archive)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("failed to start local Windows payload packager: {err}"),
            )
        })?;
    if let Err(err) = job.assign(&child) {
        let _ = child.kill();
        let _ = crate::platform::wait_child_bounded(&mut child, Duration::from_secs(5));
        return Err(io::Error::new(
            err.kind(),
            format!("failed to contain local Windows payload packager: {err}"),
        ));
    }
    let status = match crate::platform::wait_child_bounded(&mut child, Duration::from_secs(120)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            return match job.terminate_and_wait(&mut child, Duration::from_secs(5)) {
                Ok(()) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "local Windows payload packaging exceeded 120 seconds",
                )),
                Err(err) => Err(io::Error::other(format!(
                    "local Windows payload packaging timed out and cleanup failed: {err}"
                ))),
            };
        }
        Err(wait_err) => {
            return match job.terminate_and_wait(&mut child, Duration::from_secs(5)) {
                Ok(()) => Err(io::Error::new(
                    wait_err.kind(),
                    format!("failed to wait for local Windows payload packager: {wait_err}"),
                )),
                Err(cleanup_err) => Err(io::Error::other(format!(
                    "failed to wait for local Windows payload packager ({wait_err}); cleanup also failed: {cleanup_err}"
                ))),
            };
        }
    };
    if !status.success() {
        return Err(io::Error::other(format!(
            "local Windows payload packager exited with {status}"
        )));
    }
    validate_windows_zip_path(archive)
}

#[cfg(windows)]
fn package_local_windows_runtime(executable: &Path) -> io::Result<InstallSource> {
    let source_root = local_windows_runtime_root(executable)?;
    let executable_sha256 = file_sha256(executable)?;
    let runtime_executable = source_root.join("herdr.exe");
    if file_sha256(&runtime_executable)? != executable_sha256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local Windows portable runtime herdr.exe does not match the running executable",
        ));
    }
    let temporary_dir = private_download_dir("windows-local-payload")?;
    let stage = temporary_dir.join("stage");
    let archive = temporary_dir.join("payload.zip");
    let result = (|| {
        run_local_windows_payload_packager(source_root, &stage, &archive)?;
        let packaged_executable_sha256 = file_sha256(&stage.join("herdr.exe"))?;
        if packaged_executable_sha256 != executable_sha256 {
            return Err(io::Error::other(
                "local Windows remote executable changed while packaging",
            ));
        }
        let archive_sha256 = file_sha256(&archive)?;
        Ok(InstallSource::local_windows_zip(
            archive.clone(),
            temporary_dir.clone(),
            archive_sha256,
            executable_sha256,
        ))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary_dir);
    }
    result
}

#[cfg(not(windows))]
fn package_local_windows_runtime(_executable: &Path) -> io::Result<InstallSource> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "a local Windows runtime can only be packaged by a Windows client",
    ))
}

fn local_binary_can_seed_remote(platform: &RemotePlatform) -> bool {
    if !local_binary_compatible_with_remote(platform) {
        return false;
    }

    std::env::current_exe()
        .map(|path| {
            !crate::update::is_package_manager_managed_exe_path(&path)
                && (platform.os != "windows" || local_windows_runtime_root(&path).is_ok())
        })
        .unwrap_or(false)
}

fn local_binary_compatible_with_remote(platform: &RemotePlatform) -> bool {
    let local = RemotePlatform::local();
    *platform == local
        || (local.os == "windows"
            && local.arch == "x86_64"
            && platform.os == "windows"
            && platform.arch == "aarch64")
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteServerStatus {
    Running {
        version: Option<String>,
        protocol: Option<u32>,
        binary: Option<String>,
        live_handoff: bool,
        detached_server_daemon: bool,
    },
    NotRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteServerRestartReason {
    ProtocolMismatch,
    DaemonDetachMissing,
    BinaryUpdated,
    VersionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteInstallRunningServerPlan {
    KeepRunning,
    LiveHandoff,
    StopRequired(RemoteServerRestartReason),
}

fn ensure_remote_server_ready(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    remote_binary_changed: bool,
    stop_after_install_approved: bool,
    live_handoff_enabled: bool,
    known_status: Option<RemoteServerStatus>,
    restart_required: bool,
) -> io::Result<()> {
    ssh.progress(format_args!(
        "Checking the Herdr server on {}...",
        ssh.target()
    ));
    let status = match known_status {
        Some(status) => status,
        None => remote_server_status(ssh, remote_herdr)?,
    };
    let RemoteServerStatus::Running {
        version,
        protocol,
        binary: _,
        live_handoff,
        detached_server_daemon,
    } = status
    else {
        return Ok(());
    };

    let Some(reason) = remote_server_restart_reason(
        version.as_deref(),
        protocol,
        detached_server_daemon,
        remote_binary_changed,
    ) else {
        return Ok(());
    };

    if live_handoff_enabled && live_handoff {
        ssh.progress(format_args!(
            "Handing the Herdr server on {} to the new binary...",
            ssh.target()
        ));
        match live_handoff_remote_server(ssh, remote_herdr) {
            Ok(()) => return Ok(()),
            Err(err) => {
                eprintln!("remote live handoff failed: {err}");
                eprintln!("falling back to remote server restart.");
            }
        }
    }

    if stop_after_install_approved {
        stop_remote_server(ssh, remote_herdr)?;
        return Ok(());
    }

    if confirm_remote_server_stop(ssh.target(), version.as_deref(), protocol, reason)? {
        stop_remote_server(ssh, remote_herdr)?;
    } else if restart_required {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "the Windows remote server must restart before this client can attach",
        ));
    }
    Ok(())
}

fn remote_server_restart_reason(
    version: Option<&str>,
    protocol: Option<u32>,
    detached_server_daemon: bool,
    remote_binary_changed: bool,
) -> Option<RemoteServerRestartReason> {
    if protocol != Some(CURRENT_PROTOCOL) {
        return Some(RemoteServerRestartReason::ProtocolMismatch);
    }
    if !detached_server_daemon {
        return Some(RemoteServerRestartReason::DaemonDetachMissing);
    }
    if version != Some(current_version().as_str()) {
        return Some(RemoteServerRestartReason::VersionMismatch);
    }
    if remote_binary_changed {
        return Some(RemoteServerRestartReason::BinaryUpdated);
    }
    None
}

fn confirm_remote_install_with_running_server(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    live_handoff_enabled: bool,
) -> io::Result<bool> {
    let target = ssh.target();
    let status = match remote_server_status(ssh, remote_herdr) {
        Ok(status) => status,
        Err(err) => {
            if !io::stdin().is_terminal() {
                return Err(io::Error::other(format!(
                    "could not inspect the running remote herdr server on {target} before installing: {err}; run from an interactive terminal to approve updating the remote binary"
                )));
            }
            eprintln!(
                "could not inspect the running remote herdr server on {target} before installing: {err}"
            );
            eprint!("continue installing the remote herdr binary? [y/N] ");
            io::stderr().flush()?;

            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            let answer = answer.trim().to_ascii_lowercase();
            if answer != "y" && answer != "yes" {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "remote herdr install cancelled",
                ));
            }
            return Ok(false);
        }
    };
    let RemoteServerStatus::Running {
        version,
        protocol,
        binary: _,
        live_handoff,
        detached_server_daemon,
    } = &status
    else {
        return Ok(false);
    };
    let plan = remote_install_running_server_plan(
        version.as_deref(),
        *protocol,
        *detached_server_daemon,
        true,
        *live_handoff,
        live_handoff_enabled,
    );

    if plan == RemoteInstallRunningServerPlan::KeepRunning {
        if io::stdin().is_terminal() {
            eprintln!("remote herdr server on {target} is already compatible:");
            eprintln!("  server: v{}", version_label(version.as_deref()));
            eprintln!(
                "Herdr will install {} without stopping the running remote server.",
                current_version()
            );
        }
        return Ok(false);
    }

    if !io::stdin().is_terminal() {
        match plan {
            RemoteInstallRunningServerPlan::LiveHandoff => return Ok(false),
            RemoteInstallRunningServerPlan::StopRequired(_) => {
                return Err(io::Error::other(format!(
                    "remote herdr server on {target} is running v{}; run from an interactive terminal to approve stopping it for the update",
                    version_label(version.as_deref())
                )));
            }
            RemoteInstallRunningServerPlan::KeepRunning => return Ok(false),
        }
    }

    if plan == RemoteInstallRunningServerPlan::LiveHandoff {
        eprintln!("remote herdr server on {target} is currently running:");
        eprintln!("  server: v{}", version_label(version.as_deref()));
        eprintln!(
            "Herdr will install {} and hand off live pane processes to the prepared server.",
            current_version()
        );
        return Ok(false);
    }

    eprintln!("remote herdr server on {target} is currently running:");
    eprintln!("  server: v{}", version_label(version.as_deref()));
    eprintln!(
        "To complete the remote update, Herdr must stop the running remote server after installing."
    );
    eprintln!("This stops active remote pane processes, including shells, dev servers, and tests.");
    eprintln!();
    eprint!(
        "Install {} and stop the remote server now? [y/N] ",
        current_version()
    );
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer != "y" && answer != "yes" {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote herdr install cancelled",
        ));
    }

    Ok(true)
}

fn remote_install_running_server_plan(
    version: Option<&str>,
    protocol: Option<u32>,
    detached_server_daemon: bool,
    remote_binary_changed: bool,
    live_handoff: bool,
    live_handoff_enabled: bool,
) -> RemoteInstallRunningServerPlan {
    let Some(reason) = remote_server_restart_reason(
        version,
        protocol,
        detached_server_daemon,
        remote_binary_changed,
    ) else {
        return RemoteInstallRunningServerPlan::KeepRunning;
    };

    if live_handoff_enabled && live_handoff {
        return RemoteInstallRunningServerPlan::LiveHandoff;
    }

    RemoteInstallRunningServerPlan::StopRequired(reason)
}

fn provision_remote(
    ssh: &RemoteSsh,
    detected: DetectedRemoteHost,
    yes: bool,
) -> io::Result<RemoteProvisionResult> {
    if !yes && !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "non-interactive remote provisioning requires --yes",
        ));
    }
    let DetectedRemoteHost {
        host,
        warm_windows_attach,
    } = detected;
    let (platform, prepared) = match host {
        RemoteHostPlatform::Unix(platform) => {
            let prepared = prepare_remote_herdr(ssh, platform.clone(), false, yes)?;
            (platform, prepared)
        }
        RemoteHostPlatform::Windows {
            platform,
            user_profile,
            ..
        } => {
            let prepared = match warm_windows_attach {
                Some((remote_herdr, _)) => PreparedRemoteHerdr {
                    remote_herdr,
                    installed_or_replaced: false,
                    stop_after_install_approved: false,
                },
                None => prepare_remote_windows_herdr(ssh, platform.clone(), &user_profile, yes)?,
            };
            (platform, prepared)
        }
    };
    validate_remote_config(ssh, &prepared.remote_herdr)?;
    let status = remote_server_status(ssh, &prepared.remote_herdr)?;
    let stopped_for_install = prepared.stop_after_install_approved;
    let server_outcome = activate_provisioned_remote(
        ssh,
        &prepared.remote_herdr,
        prepared.installed_or_replaced,
        yes,
        status,
    )?;
    let server_outcome = match (stopped_for_install, server_outcome) {
        (true, RemoteServerOutcome::Started) => RemoteServerOutcome::Restarted,
        (_, outcome) => outcome,
    };

    Ok(RemoteProvisionResult {
        target: ssh.target().to_string(),
        platform: platform.asset_key(),
        binary: prepared.remote_herdr.shell_path,
        binary_outcome: if prepared.installed_or_replaced {
            RemoteBinaryOutcome::Installed
        } else {
            RemoteBinaryOutcome::AlreadyMatching
        },
        server_outcome,
        version: current_version(),
        protocol: CURRENT_PROTOCOL,
    })
}

fn validate_remote_config(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    ssh.progress(format_args!(
        "Checking the Herdr configuration on {}...",
        ssh.target()
    ));
    let output = remote_herdr_output(ssh, remote_herdr, &["config", "check"])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(
            "remote Herdr configuration is invalid",
            &output,
        ))
    }
}

fn activate_provisioned_remote(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    binary_changed: bool,
    yes: bool,
    status: RemoteServerStatus,
) -> io::Result<RemoteServerOutcome> {
    let binary_matches = if binary_changed {
        false
    } else if let RemoteServerStatus::Running {
        binary: Some(running_binary),
        ..
    } = &status
    {
        let selected_binary = remote_client_binary(ssh, remote_herdr)?;
        remote_binary_paths_match_for(remote_herdr.shell, running_binary, &selected_binary)
    } else {
        false
    };
    match remote_provision_server_action(&status, binary_changed, binary_matches) {
        RemoteProvisionServerAction::Start => {
            start_remote_server(ssh, remote_herdr)?;
            Ok(RemoteServerOutcome::Started)
        }
        RemoteProvisionServerAction::Reload => {
            reload_remote_config(ssh, remote_herdr)?;
            Ok(RemoteServerOutcome::Reloaded)
        }
        RemoteProvisionServerAction::Restart => {
            if !yes && !confirm_remote_provision_restart(ssh.target(), &status)? {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "remote Herdr server restart cancelled",
                ));
            }
            stop_remote_server(ssh, remote_herdr)?;
            start_remote_server(ssh, remote_herdr)?;
            Ok(RemoteServerOutcome::Restarted)
        }
    }
}

fn remote_provision_server_action(
    status: &RemoteServerStatus,
    binary_changed: bool,
    binary_matches: bool,
) -> RemoteProvisionServerAction {
    match status {
        RemoteServerStatus::NotRunning => RemoteProvisionServerAction::Start,
        RemoteServerStatus::Running {
            version,
            protocol: Some(CURRENT_PROTOCOL),
            binary: Some(binary),
            detached_server_daemon: true,
            ..
        } if !binary_changed
            && version.as_deref() == Some(current_version().as_str())
            && binary_matches
            && !binary.is_empty() =>
        {
            RemoteProvisionServerAction::Reload
        }
        RemoteServerStatus::Running { .. } => RemoteProvisionServerAction::Restart,
    }
}

fn remote_client_binary(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<String> {
    let output = remote_herdr_output(ssh, remote_herdr, &["status", "client", "--json"])?;
    if !output.status.success() {
        return Err(command_failed(
            "remote client binary identity check failed",
            &output,
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status: RemoteClientStatusJson = serde_json::from_str(stdout.trim()).map_err(|err| {
        io::Error::other(format!(
            "could not parse remote client status JSON from `{}`: {err}",
            stdout.trim()
        ))
    })?;
    status.binary.ok_or_else(|| {
        io::Error::other("remote client status did not report its executable identity")
    })
}

fn remote_binary_paths_match_for(shell: RemoteShell, running: &str, selected: &str) -> bool {
    match shell {
        RemoteShell::WindowsPowerShell => remote_binary_paths_match(running, selected),
        RemoteShell::Posix => running == selected,
    }
}

fn confirm_remote_provision_restart(target: &str, status: &RemoteServerStatus) -> io::Result<bool> {
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(format!(
            "remote Herdr server on {target} must restart to activate the provisioned binary; rerun with --yes to approve stopping its pane processes"
        )));
    }
    let version = match status {
        RemoteServerStatus::Running { version, .. } => version_label(version.as_deref()),
        RemoteServerStatus::NotRunning => "not running",
    };
    eprintln!("remote Herdr server on {target} is running v{version}.");
    eprintln!("restarting saves the Herdr session but stops its active pane processes.");
    eprint!("restart the remote server now? [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[derive(Debug, Deserialize)]
struct RemoteConfigReloadJson {
    status: RemoteConfigReloadStatus,
    #[serde(default)]
    diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RemoteConfigReloadStatus {
    Applied,
    Partial,
    Failed,
}

fn reload_remote_config(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    ssh.progress(format_args!(
        "Applying the Herdr configuration on {}...",
        ssh.target()
    ));
    let output = remote_herdr_output(ssh, remote_herdr, &["server", "reload-config", "--json"])?;
    if !output.status.success() {
        return Err(command_failed(
            "remote server config reload failed",
            &output,
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: RemoteConfigReloadJson = serde_json::from_str(stdout.trim()).map_err(|err| {
        io::Error::other(format!(
            "could not parse remote config reload JSON from `{}`: {err}",
            stdout.trim()
        ))
    })?;
    if result.status == RemoteConfigReloadStatus::Applied {
        ssh.progress(format_args!(
            "The Herdr configuration is active on {}.",
            ssh.target()
        ));
        return Ok(());
    }
    Err(io::Error::other(format!(
        "remote server config reload was {:?}: {}",
        result.status,
        result.diagnostics.join("; ")
    )))
}

fn start_remote_server(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    ssh.progress(format_args!(
        "Starting the Herdr server on {}...",
        ssh.target()
    ));
    let output = remote_herdr_output(ssh, remote_herdr, &["server", "start"])?;
    if !output.status.success() {
        return Err(command_failed("remote server start failed", &output));
    }
    let status = remote_server_status(ssh, remote_herdr)?;
    let selected_binary = remote_client_binary(ssh, remote_herdr)?;
    match status {
        RemoteServerStatus::Running {
            version,
            protocol: Some(CURRENT_PROTOCOL),
            binary: Some(running_binary),
            detached_server_daemon: true,
            ..
        } if version.as_deref() == Some(current_version().as_str())
            && remote_binary_paths_match_for(
                remote_herdr.shell,
                &running_binary,
                &selected_binary,
            ) =>
        {
            ssh.progress(format_args!(
                "The Herdr server is running on {}.",
                ssh.target()
            ));
            Ok(())
        }
        _ => Err(io::Error::other(
            "remote server did not report the provisioned binary, version, and protocol after start",
        )),
    }
}

fn print_remote_provision_result(result: &RemoteProvisionResult, json: bool) -> io::Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string(result).map_err(io::Error::other)?
        );
    } else {
        println!("remote provision: ok");
        println!("target: {}", result.target);
        println!("platform: {}", result.platform);
        println!("binary: {}", result.binary);
        println!("binary outcome: {:?}", result.binary_outcome);
        println!("server outcome: {:?}", result.server_outcome);
        println!("version: {}", result.version);
        println!("protocol: {}", result.protocol);
    }
    Ok(())
}

fn remote_server_status(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
) -> io::Result<RemoteServerStatus> {
    let output = remote_herdr_output(ssh, remote_herdr, &["status", "server", "--json"])?;
    if !output.status.success() {
        return Err(command_failed("remote server status failed", &output));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_remote_server_status_json(stdout.trim())
}

#[derive(Debug, Deserialize)]
struct RemoteClientStatusJson {
    version: String,
    protocol: u32,
    #[serde(default)]
    binary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WindowsAttachProbeJson {
    os: String,
    arch: String,
    user_profile: String,
    default_shell: String,
    selected: Option<WindowsAttachSelectionJson>,
}

#[derive(Debug, Deserialize)]
struct WindowsAttachSelectionJson {
    path: String,
    sidecar: bool,
    client: RemoteClientStatusJson,
    server: RemoteServerStatusJson,
}

#[derive(Debug, Deserialize)]
struct RemoteServerStatusJson {
    running: bool,
    version: Option<String>,
    protocol: Option<u32>,
    #[serde(default)]
    binary: Option<String>,
    capabilities: Option<RemoteServerCapabilitiesJson>,
}

#[derive(Debug, Deserialize)]
struct RemoteServerCapabilitiesJson {
    live_handoff: bool,
    #[serde(default)]
    detached_server_daemon: bool,
}

fn parse_client_status_json(status: &str) -> Option<RemoteClientStatusJson> {
    serde_json::from_str(status).ok()
}

fn remote_client_status_matches_current(status: &str) -> bool {
    parse_client_status_json(status).is_some_and(|status| {
        status.version == current_version() && status.protocol == CURRENT_PROTOCOL
    })
}

fn parse_remote_server_status_json(status: &str) -> io::Result<RemoteServerStatus> {
    let parsed: RemoteServerStatusJson = serde_json::from_str(status).map_err(|err| {
        io::Error::other(format!(
            "could not parse remote server status JSON from `{status}`: {err}"
        ))
    })?;
    Ok(remote_server_status_from_json(parsed))
}

fn remote_server_status_from_json(parsed: RemoteServerStatusJson) -> RemoteServerStatus {
    if !parsed.running {
        return RemoteServerStatus::NotRunning;
    }

    let capabilities = parsed.capabilities;

    RemoteServerStatus::Running {
        version: parsed.version,
        protocol: parsed.protocol,
        binary: parsed.binary,
        live_handoff: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.live_handoff),
        detached_server_daemon: capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.detached_server_daemon),
    }
}

fn confirm_remote_server_stop(
    target: &str,
    version: Option<&str>,
    _protocol: Option<u32>,
    reason: RemoteServerRestartReason,
) -> io::Result<bool> {
    if !io::stdin().is_terminal() {
        if reason == RemoteServerRestartReason::ProtocolMismatch {
            return Err(io::Error::other(format!(
                "remote herdr server on {target} must stop before this client can attach; run from an interactive terminal to approve stopping it"
            )));
        }

        eprintln!(
            "remote herdr server on {target} is still running v{}; it will use {} after it restarts.",
            version_label(version),
            current_version()
        );
        return Ok(false);
    }

    eprintln!("remote herdr server on {target} is currently running:");
    eprintln!("  server: v{}", version_label(version));
    eprintln!("  prepared binary: {}", current_version());
    eprintln!();

    match reason {
        RemoteServerRestartReason::ProtocolMismatch => {
            eprintln!("the remote server must stop before this client can attach.");
        }
        RemoteServerRestartReason::DaemonDetachMissing => {
            eprintln!(
                "the remote server was started by a herdr build that may not survive SSH connection loss. restart it so network drops disconnect only this client."
            );
        }
        RemoteServerRestartReason::BinaryUpdated => {
            eprintln!(
                "the remote herdr binary was installed or replaced. restart the remote server so it uses the prepared binary."
            );
        }
        RemoteServerRestartReason::VersionMismatch => {
            eprintln!(
                "the remote server is still running a different herdr version. restart it so it uses the prepared binary."
            );
        }
    }

    let prompt = if reason == RemoteServerRestartReason::ProtocolMismatch {
        "stop the remote server and continue attaching? [Y/n] "
    } else {
        "restart the remote server now? [y/N] "
    };
    eprint!("{prompt}");
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        return Ok(true);
    }
    if answer.is_empty() && reason == RemoteServerRestartReason::ProtocolMismatch {
        return Ok(true);
    }
    if reason == RemoteServerRestartReason::ProtocolMismatch {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote herdr server stop cancelled",
        ));
    }

    Ok(false)
}

fn live_handoff_remote_server(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    let command = format!(
        "{} server live-handoff --import-exe {} --expected-protocol {} --expected-version {}",
        remote_herdr.shell_path,
        remote_herdr.shell_path,
        CURRENT_PROTOCOL,
        current_version()
    );
    let output = ssh.sh_output(&command)?;
    if !output.status.success() {
        return Err(command_failed("remote server live handoff failed", &output));
    }

    ssh.progress(format_args!(
        "The Herdr server on {} is using the new binary. Reconnecting...",
        ssh.target()
    ));
    Ok(())
}

fn stop_remote_server(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    ssh.progress(format_args!(
        "Stopping the Herdr server on {}...",
        ssh.target()
    ));
    let output = remote_herdr_output(ssh, remote_herdr, &["server", "stop"])?;
    if !output.status.success() {
        return Err(command_failed("remote server stop failed", &output));
    }

    wait_for_remote_server_shutdown(ssh, remote_herdr)?;
    ssh.progress(format_args!(
        "The Herdr server is stopped on {}.",
        ssh.target()
    ));
    Ok(())
}

fn wait_for_remote_server_shutdown(ssh: &RemoteSsh, remote_herdr: &RemoteHerdr) -> io::Result<()> {
    let deadline = Instant::now() + REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT;
    loop {
        if remote_server_status(ssh, remote_herdr)? == RemoteServerStatus::NotRunning {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "shutdown was requested, but the old remote herdr server on {target} is still responding after {} seconds",
                    REMOTE_SERVER_SHUTDOWN_CONFIRM_TIMEOUT.as_secs(),
                    target = ssh.target()
                ),
            ));
        }
        thread::sleep(REMOTE_SERVER_SHUTDOWN_POLL_INTERVAL);
    }
}

fn version_label(version: Option<&str>) -> &str {
    version.unwrap_or("unknown")
}

fn warn_if_remote_bin_not_on_path(ssh: &RemoteSsh) -> io::Result<()> {
    let output = ssh.user_shell_output("command -v herdr")?;
    if output.status.success()
        && remote_shell_resolves_managed_install(&String::from_utf8_lossy(&output.stdout))
    {
        return Ok(());
    }

    eprintln!(
        "herdr: installed remote binary to ~/.local/bin/herdr, but the remote shell does not resolve `herdr` to that path"
    );
    Ok(())
}

fn remote_shell_resolves_managed_install(stdout: &str) -> bool {
    stdout
        .lines()
        .next()
        .map(str::trim)
        .is_some_and(|path| path.ends_with("/.local/bin/herdr"))
}

fn download_release_asset(platform: &RemotePlatform) -> io::Result<InstallSource> {
    let asset_key = platform.asset_key();
    let asset = remote_release_asset(&asset_key)?;

    let dir = private_download_dir(&asset_key)?;
    let path = dir.join("herdr.tmp");
    let status = crate::noninteractive_process::curl_command(&asset.url)
        .args(["--max-time", "120", "-o"])
        .arg(&path)
        .status()
        .map_err(|err| io::Error::new(err.kind(), format!("download failed: {err}")))?;
    if !status.success() {
        let _ = fs::remove_dir_all(&dir);
        return Err(io::Error::other("download failed"));
    }
    let windows_zip = platform.os == "windows";
    if windows_zip && asset.sha256.is_none() {
        let _ = fs::remove_dir_all(&dir);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows remote portable asset is missing its SHA-256 digest",
        ));
    }
    if windows_zip && asset.format.as_deref() != Some("zip") {
        let _ = fs::remove_dir_all(&dir);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows remote portable asset must declare ZIP format",
        ));
    }
    if let Some(expected) = &asset.sha256 {
        if let Err(err) = crate::checksum::verify_sha256(&path, expected) {
            let _ = fs::remove_dir_all(&dir);
            return Err(io::Error::new(
                err.kind(),
                format!("downloaded remote asset checksum verification failed: {err}"),
            ));
        }
    }

    if windows_zip {
        validate_windows_zip_path(&path)?;
        let sha256 = asset.sha256.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Windows remote portable asset is missing its SHA-256 digest",
            )
        })?;
        Ok(InstallSource::windows_zip(path, Some(dir), sha256))
    } else {
        Ok(InstallSource::temporary(path, dir))
    }
}

fn fetch_remote_manifest(url: &str) -> io::Result<Vec<u8>> {
    let output = crate::noninteractive_process::curl_command(url)
        .args([
            "-H",
            "Cache-Control: no-cache",
            "--retry",
            "3",
            "--connect-timeout",
            "10",
            "--max-time",
            "20",
        ])
        .output()
        .map_err(|err| io::Error::new(err.kind(), format!("curl failed: {err}")))?;
    if !output.status.success() {
        return Err(command_failed("failed to fetch update manifest", &output));
    }
    Ok(output.stdout)
}

fn remote_asset_info(asset: &RemoteAssetRef) -> RemoteReleaseAsset {
    RemoteReleaseAsset {
        url: asset.url().to_string(),
        sha256: asset.sha256().map(str::to_string),
        format: asset.format().map(str::to_string),
    }
}

fn preview_assets_for_build<'a>(
    manifest: &'a RemotePreviewManifest,
    build_id: &str,
) -> io::Result<(u32, &'a BTreeMap<String, RemoteAssetRef>)> {
    if manifest.prerelease {
        return Err(io::Error::other(
            "update manifest is marked as a GitHub prerelease",
        ));
    }
    if manifest.build_id == build_id {
        return Ok((manifest.protocol, &manifest.assets));
    }
    let build = manifest.builds.get(build_id).ok_or_else(|| {
        io::Error::other(format!(
            "preview manifest no longer includes build {build_id}; run `herdr update` locally or set {REMOTE_BINARY_ENV_VAR}=target/release/herdr"
        ))
    })?;
    Ok((build.protocol, &build.assets))
}

fn remote_release_asset(asset_key: &str) -> io::Result<RemoteReleaseAsset> {
    if crate::build_info::is_preview() {
        let build_id = crate::build_info::build_id().ok_or_else(|| {
            io::Error::other("preview client has no build id; set HERDR_REMOTE_BINARY or install Herdr on the remote manually")
        })?;
        let manifest_bytes = fetch_remote_manifest(preview_update_manifest_url())?;
        let manifest: RemotePreviewManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|err| {
                io::Error::other(format!("failed to parse preview manifest JSON: {err}"))
            })?;
        let (protocol, assets) = preview_assets_for_build(&manifest, build_id)?;
        if protocol != CURRENT_PROTOCOL {
            return Err(io::Error::other(format!(
                "preview manifest has build {build_id} protocol {protocol}, but this client needs protocol {CURRENT_PROTOCOL}; set {REMOTE_BINARY_ENV_VAR}=target/release/herdr or install a matching Herdr on the remote host manually"
            )));
        }
        return assets.get(asset_key).map(remote_asset_info).ok_or_else(|| {
            io::Error::other(format!(
                "no {asset_key} binary in the preview manifest for build {build_id}"
            ))
        });
    }

    let current_version = current_version();
    let manifest_bytes = fetch_remote_manifest(crate::distribution::STABLE_MANIFEST_URL)?;
    let manifest: RemoteUpdateManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|err| io::Error::other(format!("failed to parse update manifest JSON: {err}")))?;
    let release = manifest.release_for_version(&current_version).ok_or_else(|| {
        io::Error::other(format!(
            "release manifest does not include herdr {current_version}; build herdr for {} or install it there manually",
            asset_key
        ))
    })?;
    if let Some(protocol) = release.protocol {
        if protocol != CURRENT_PROTOCOL {
            return Err(io::Error::other(format!(
                "release manifest has herdr {current_version} protocol {protocol}, but this client needs protocol {CURRENT_PROTOCOL}; set {REMOTE_BINARY_ENV_VAR}=target/release/herdr or install a matching herdr on the remote host manually"
            )));
        }
    }
    let asset = release.assets.get(asset_key).ok_or_else(|| {
        io::Error::other(format!(
            "no {asset_key} binary in the release manifest for herdr {current_version}"
        ))
    })?;
    let mut asset = remote_asset_info(asset);
    asset.sha256 = asset
        .sha256
        .or_else(|| release.sha256.get(asset_key).cloned());
    if asset.sha256.is_none() {
        return Err(io::Error::other(format!(
            "release manifest asset {asset_key} is missing a SHA-256 checksum"
        )));
    }
    Ok(asset)
}

fn private_download_dir(asset_key: &str) -> io::Result<PathBuf> {
    let base = crate::platform::remote_private_temp_base();
    fs::create_dir_all(&base)?;
    for attempt in 0..100 {
        let dir = base.join(format!(
            "herdr-remote-{}-{}-{attempt}",
            std::process::id(),
            asset_key
        ));
        match crate::platform::create_remote_private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to create private herdr remote download directory",
    ))
}

fn confirm_remote_install(
    target: &str,
    remote_herdr: &RemoteHerdr,
    source_description: &str,
    yes: bool,
) -> io::Result<()> {
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(format!(
            "matching remote herdr {} is not installed at {}; run from an interactive terminal to approve installation",
            current_version(),
            remote_herdr.shell_path
        )));
    }

    eprintln!(
        "matching herdr {} is not installed on {target} for {}.",
        current_version(),
        remote_herdr.platform.asset_key()
    );
    eprint!(
        "Install {} to {}? [Y/n] ",
        source_description, remote_herdr.shell_path
    );
    io::stderr().flush()?;

    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer == "n" || answer == "no" {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "remote herdr installation cancelled",
        ));
    }

    Ok(())
}

fn remote_bridge_command(
    remote_herdr: &RemoteHerdr,
    session_name: &str,
    windows_ssh_shell: Option<&super::windows::WindowsSshShell>,
) -> io::Result<String> {
    match remote_herdr.shell {
        RemoteShell::Posix => {
            let mut command = format!("exec {}", remote_herdr.shell_path);
            if session_name != crate::session::DEFAULT_SESSION_NAME {
                command.push_str(" --session ");
                command.push_str(&shell_quote(session_name));
            }
            command.push_str(" remote-client-bridge");
            Ok(command)
        }
        RemoteShell::WindowsPowerShell => {
            let mut arguments = Vec::new();
            if session_name != crate::session::DEFAULT_SESSION_NAME {
                arguments.push("--session".to_string());
                arguments.push(session_name.to_string());
            }
            arguments.push("remote-client-bridge".to_string());
            super::windows::streaming_herdr_command(
                &remote_herdr.shell_path,
                &arguments,
                remote_herdr.remote_sidecar,
                windows_ssh_shell.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Windows remote bridge is missing the OpenSSH default shell",
                    )
                })?,
            )
        }
    }
}

fn remote_herdr_output(
    ssh: &RemoteSsh,
    remote_herdr: &RemoteHerdr,
    arguments: &[&str],
) -> io::Result<Output> {
    let session_name = crate::session::active_name();
    let scoped_arguments = scoped_remote_arguments(session_name.as_deref(), arguments);
    match remote_herdr.shell {
        RemoteShell::Posix => {
            let command = std::iter::once(remote_herdr.shell_path.clone())
                .chain(
                    scoped_arguments
                        .iter()
                        .map(|argument| shell_quote(argument)),
                )
                .collect::<Vec<_>>()
                .join(" ");
            ssh.sh_output(&command)
        }
        RemoteShell::WindowsPowerShell => ssh.windows_herdr_output(remote_herdr, &scoped_arguments),
    }
}

fn scoped_remote_arguments(session_name: Option<&str>, arguments: &[&str]) -> Vec<String> {
    let mut scoped = Vec::new();
    if let Some(session_name) =
        session_name.filter(|name| *name != crate::session::DEFAULT_SESSION_NAME)
    {
        scoped.push("--session".to_string());
        scoped.push(session_name.to_string());
    }
    scoped.extend(arguments.iter().map(|argument| argument.to_string()));
    scoped
}

fn remote_binary_paths_match(running: &str, selected: &str) -> bool {
    normalize_windows_binary_path(running)
        .eq_ignore_ascii_case(&normalize_windows_binary_path(selected))
}

fn normalize_windows_binary_path(path: &str) -> String {
    let normalized = path.trim().replace('/', "\\");
    normalized
        .strip_prefix(r"\\?\")
        .unwrap_or(&normalized)
        .to_string()
}

fn reattach_command(
    program: &str,
    target: &str,
    session_name: &str,
    keybindings: RemoteKeybindings,
    live_handoff: bool,
) -> String {
    let program = crate::platform::remote_reattach_program(program);
    let target = crate::platform::remote_reattach_argument(target);
    let mut command = format!("{program} --remote {target}");
    if keybindings != RemoteKeybindings::Local {
        command.push_str(" --remote-keybindings ");
        command.push_str(keybindings.as_str());
    }
    if live_handoff {
        command.push_str(" --handoff");
    }
    if session_name != crate::session::DEFAULT_SESSION_NAME {
        command.push_str(" --session ");
        command.push_str(&crate::platform::remote_reattach_argument(session_name));
    }
    command
}

fn command_failed(context: &str, output: &Output) -> io::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        io::Error::other(format!("{context}: {}", output.status))
    } else {
        io::Error::other(format!("{context}: {stderr}"))
    }
}

struct SshStdioBridge {
    local_socket: PathBuf,
    socket_identity: crate::ipc::SocketFileIdentity,
    should_stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SshStdioBridge {
    fn start(
        target: String,
        remote_command: String,
        local_socket: PathBuf,
        ssh_options: Option<&ManagedSshOptions>,
    ) -> io::Result<Self> {
        crate::ipc::prepare_socket_path(&local_socket, |path| {
            format!("remote bridge is already listening at {}", path.display())
        })?;
        let listener = crate::ipc::bind_private_local_listener(&local_socket)?;
        let socket_identity = crate::ipc::socket_file_identity(&local_socket)?;
        if let Err(err) =
            crate::ipc::restrict_socket_permissions(&local_socket, BRIDGE_SOCKET_PERMISSION_MODE)
        {
            let _ = crate::ipc::remove_socket_file_if_owned(&local_socket, &socket_identity);
            return Err(err);
        }
        if let Err(err) = listener.set_nonblocking(ListenerNonblockingMode::Accept) {
            let _ = crate::ipc::remove_socket_file_if_owned(&local_socket, &socket_identity);
            return Err(err);
        }

        let should_stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&should_stop);
        let thread_ssh_options = ssh_options.cloned();
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok(stream) => {
                        let stream = match prepare_remote_bridge_stream(stream) {
                            Ok(stream) => stream,
                            Err(err) => {
                                tracing::error!(
                                    error = %err,
                                    "remote bridge failed to prepare client socket"
                                );
                                continue;
                            }
                        };
                        if let Err(err) = bridge_connection(
                            stream,
                            &target,
                            &remote_command,
                            thread_ssh_options.as_ref(),
                            &thread_stop,
                        ) {
                            eprintln!("herdr: remote bridge failed: {err}");
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(BRIDGE_ACCEPT_POLL);
                    }
                    Err(err) => {
                        eprintln!("herdr: remote bridge listener failed: {err}");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            local_socket,
            socket_identity,
            should_stop,
            thread: Some(thread),
        })
    }
}

fn prepare_remote_bridge_stream(
    mut stream: crate::ipc::LocalStream,
) -> io::Result<crate::ipc::LocalStream> {
    crate::ipc::set_local_stream_polling(&mut stream, false)?;
    Ok(stream)
}

impl Drop for SshStdioBridge {
    fn drop(&mut self) {
        self.should_stop.store(true, Ordering::Release);
        #[cfg(unix)]
        let _ = crate::ipc::remove_socket_file_if_owned(&self.local_socket, &self.socket_identity);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        #[cfg(windows)]
        let _ = crate::ipc::remove_socket_file_if_owned(&self.local_socket, &self.socket_identity);
    }
}

fn ssh_config_quote(path: &str) -> String {
    format!("\"{path}\"")
}

fn ssh_config_include_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '\\' {
        ssh_config_quote(&path.replace('\\', "/"))
    } else {
        ssh_config_quote(&path)
    }
}

/// Builds a temporary ssh config that includes the user's settings first, so
/// OpenSSH's first-value-wins behavior preserves explicit user keepalives.
fn write_managed_ssh_config() -> io::Result<ManagedSshConfig> {
    let paths = crate::platform::remote_ssh_config_paths();
    let dir = crate::platform::create_remote_ssh_config_dir(SSH_CONTROL_SOCKET_NAME)?;
    let path = dir.join("config");
    let control_path = paths
        .multiplexing
        .then(|| dir.join(SSH_CONTROL_SOCKET_NAME));

    let mut contents = String::new();
    if let Some(user_config) = paths.user_config.filter(|path| path.is_file()) {
        contents.push_str(&format!(
            "Include {}\n",
            ssh_config_include_path(&user_config)
        ));
    }
    if let Some(system_config) = paths.system_config.filter(|path| path.is_file()) {
        contents.push_str(&format!(
            "Include {}\n",
            ssh_config_include_path(&system_config)
        ));
    }
    contents.push_str("Host *\n");
    contents.push_str("  ServerAliveInterval 15\n");
    contents.push_str("  ServerAliveCountMax 4\n");

    let write_result = (|| {
        let mut file = crate::platform::create_remote_ssh_config_file(&path)?;
        file.write_all(contents.as_bytes())
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_dir_all(&dir);
        return Err(err);
    }
    Ok(ManagedSshConfig {
        options: ManagedSshOptions {
            config_path: path,
            control_path,
        },
    })
}

#[cfg(unix)]
fn bridge_connection(
    stream: crate::ipc::LocalStream,
    target: &str,
    remote_command: &str,
    ssh_options: Option<&ManagedSshOptions>,
    _bridge_stop: &Arc<AtomicBool>,
) -> io::Result<()> {
    let mut command = Command::new("ssh");
    apply_managed_ssh_options(&mut command, ssh_options);
    command
        .arg("-T")
        .arg(target)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("failed to start ssh bridge: {err}")))?;
    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ssh bridge stdin missing"))?;
    let mut child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "ssh bridge stdout missing"))?;
    let mut stream_to_child = stream.try_clone()?;
    let mut child_to_stream = stream;

    let upload = thread::spawn(move || {
        let _ = copy_flush(&mut stream_to_child, &mut child_stdin);
    });
    let download = thread::spawn(move || {
        let _ = copy_flush(&mut child_stdout, &mut child_to_stream);
        let _ = crate::ipc::shutdown_local_stream_write(&child_to_stream);
    });

    let status = child.wait()?;
    let _ = upload.join();
    let _ = download.join();

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            format!("ssh bridge exited with {status}"),
        ))
    }
}

#[cfg(windows)]
fn bridge_connection(
    stream: crate::ipc::LocalStream,
    target: &str,
    remote_command: &str,
    ssh_options: Option<&ManagedSshOptions>,
    bridge_stop: &Arc<AtomicBool>,
) -> io::Result<()> {
    let mut command = Command::new("ssh");
    apply_managed_ssh_options(&mut command, ssh_options);
    command
        .arg("-T")
        .arg(target)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("failed to start ssh bridge: {err}")))?;
    let mut child_stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => return terminate_bridge_child(child, "ssh bridge stdin missing"),
    };
    let mut child_stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => return terminate_bridge_child(child, "ssh bridge stdout missing"),
    };
    let stream_to_child = match stream.try_clone() {
        Ok(stream) => stream,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };
    if let Err(err) = stream.set_nonblocking(true) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }
    let mut child_to_stream = stream;

    let connection_stop = Arc::new(AtomicBool::new(false));
    let upload_stop = Arc::new(AtomicBool::new(false));
    let upload_failed = Arc::new(AtomicBool::new(false));
    let download_done = Arc::new(AtomicBool::new(false));
    let client_closed = Arc::new(AtomicBool::new(false));
    let upload_cancel = Arc::clone(&upload_stop);
    let upload_bridge_stop = Arc::clone(bridge_stop);
    let upload_failed_worker = Arc::clone(&upload_failed);
    let upload_client_closed = Arc::clone(&client_closed);
    let upload = thread::spawn(move || {
        let result = copy_local_stream_to_writer(
            stream_to_child,
            &mut child_stdin,
            &upload_cancel,
            &upload_bridge_stop,
            &upload_client_closed,
        );
        upload_failed_worker.store(result.is_err(), Ordering::Release);
        result
    });
    let download_stop = Arc::clone(&connection_stop);
    let download_bridge_stop = Arc::clone(bridge_stop);
    let download_done_worker = Arc::clone(&download_done);
    let download_upload_stop = Arc::clone(&upload_stop);
    let download = thread::spawn(move || {
        let result = copy_reader_to_local_stream(
            &mut child_stdout,
            &mut child_to_stream,
            &download_stop,
            &download_bridge_stop,
        );
        download_done_worker.store(true, Ordering::Release);
        download_upload_stop.store(true, Ordering::Release);
        result
    });

    let mut stopped_at = None;
    let (status_result, child_exited) = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                upload_stop.store(true, Ordering::Release);
                break (Ok(status), true);
            }
            Ok(None) => {}
            Err(err) => {
                connection_stop.store(true, Ordering::Release);
                upload_stop.store(true, Ordering::Release);
                let _ = child.kill();
                let _ = child.wait();
                break (Err(err), false);
            }
        }
        if bridge_stop.load(Ordering::Acquire) {
            connection_stop.store(true, Ordering::Release);
            upload_stop.store(true, Ordering::Release);
            let _ = child.kill();
            break (child.wait(), false);
        }
        if client_closed.load(Ordering::Acquire)
            || upload_failed.load(Ordering::Acquire)
            || download_done.load(Ordering::Acquire)
        {
            upload_stop.store(true, Ordering::Release);
            let stopped_at = stopped_at.get_or_insert_with(Instant::now);
            if stopped_at.elapsed() >= Duration::from_millis(250) {
                connection_stop.store(true, Ordering::Release);
                let _ = child.kill();
                break (child.wait(), false);
            }
        }
        thread::sleep(BRIDGE_ACCEPT_POLL);
    };
    upload_stop.store(true, Ordering::Release);
    if !child_exited {
        connection_stop.store(true, Ordering::Release);
    }
    let upload_result = upload
        .join()
        .map_err(|_| io::Error::other("remote bridge upload worker panicked"))?;
    let download_result = download
        .join()
        .map_err(|_| io::Error::other("remote bridge download worker panicked"))?;
    let status = status_result?;

    let stopping = bridge_stop.load(Ordering::Acquire);
    let client_closed = client_closed.load(Ordering::Acquire);
    if !stopping && !client_closed {
        upload_result.map_err(|err| {
            io::Error::new(err.kind(), format!("remote bridge upload failed: {err}"))
        })?;
        download_result.map_err(|err| {
            io::Error::new(err.kind(), format!("remote bridge download failed: {err}"))
        })?;
    }

    if status.success() || stopping || client_closed {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            format!("ssh bridge exited with {status}"),
        ))
    }
}

#[cfg(unix)]
fn copy_flush<R: io::Read, W: io::Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;

    loop {
        let bytes_read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(bytes_read) => bytes_read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };

        writer.write_all(&buffer[..bytes_read])?;
        writer.flush()?;
        total += bytes_read as u64;
    }
}

#[cfg(windows)]
fn terminate_bridge_child(mut child: std::process::Child, message: &'static str) -> io::Result<()> {
    let _ = child.kill();
    let _ = child.wait();
    Err(io::Error::new(io::ErrorKind::BrokenPipe, message))
}

#[cfg(windows)]
fn copy_reader_to_local_stream<R: io::Read>(
    reader: &mut R,
    stream: &mut crate::ipc::LocalStream,
    connection_stop: &AtomicBool,
    bridge_stop: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(read) => read,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let mut written = 0;
        while written < read {
            if connection_stop.load(Ordering::Acquire) || bridge_stop.load(Ordering::Acquire) {
                return Ok(total);
            }
            let chunk_len = (read - written).min(4 * 1024);
            match stream.write(&buffer[written..written + chunk_len]) {
                Ok(0) => thread::sleep(BRIDGE_IO_POLL),
                Ok(count) => written += count,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(BRIDGE_IO_POLL);
                }
                Err(err) => return Err(err),
            }
        }
        stream.flush()?;
        total += read as u64;
    }
}

#[cfg(windows)]
fn copy_local_stream_to_writer<W: io::Write>(
    mut stream: crate::ipc::LocalStream,
    writer: &mut W,
    connection_stop: &AtomicBool,
    bridge_stop: &AtomicBool,
    client_closed: &AtomicBool,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0;

    while !connection_stop.load(Ordering::Acquire) && !bridge_stop.load(Ordering::Acquire) {
        match crate::ipc::poll_local_stream_read_count(&mut stream, &mut buffer)? {
            crate::ipc::LocalStreamReadCount::Data(read) => {
                writer.write_all(&buffer[..read])?;
                writer.flush()?;
                total += read as u64;
            }
            crate::ipc::LocalStreamReadCount::Pending => thread::sleep(BRIDGE_IO_POLL),
            crate::ipc::LocalStreamReadCount::Closed => {
                client_closed.store(true, Ordering::Release);
                break;
            }
        }
    }

    Ok(total)
}

fn run_client_process(
    local_socket: &Path,
    reattach_command: &str,
    keybindings: RemoteKeybindings,
) -> io::Result<()> {
    let exe = crate::managed_install::command_executable()?;
    let status = Command::new(exe)
        .arg("client")
        .env(
            crate::server::socket_paths::CLIENT_SOCKET_PATH_ENV_VAR,
            local_socket,
        )
        .env("HERDR_RENDER_ENCODING", "terminal-ansi")
        .env(REATTACH_COMMAND_ENV_VAR, reattach_command)
        .env(REMOTE_KEYBINDINGS_ENV_VAR, keybindings.as_str())
        .env_remove(crate::api::SOCKET_PATH_ENV_VAR)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("remote client exited with {status}"),
        ))
    }
}

fn local_forward_socket_path(target: &str, session_name: &str) -> PathBuf {
    let pid = std::process::id();
    let target_clean = sanitize_path_component(target);
    let session_clean = sanitize_path_component(session_name);
    let readable_name = format!("herdr-remote-{pid}-{target_clean}-{session_clean}.sock");
    let target_prefix: String = target_clean.chars().take(8).collect();
    let hash = short_socket_hash(target, session_name);
    let short_name = format!("herdr-r-{pid}-{target_prefix}-{hash}.sock");
    crate::platform::remote_bridge_endpoint_path(&readable_name, &short_name)
}

#[cfg(all(test, unix))]
fn fits_unix_socket_path(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().len() <= 103
}

fn short_socket_hash(target: &str, session: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    0u8.hash(&mut hasher);
    session.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn sanitize_path_component(input: &str) -> String {
    let sanitized: String = input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '-'
            }
        })
        .collect();

    sanitized.trim_matches('-').chars().take(32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn bridge_socket_is_user_only() {
        use std::os::unix::fs::PermissionsExt;

        let socket = std::env::temp_dir().join(format!(
            "herdr-bridge-permissions-test-{}.sock",
            std::process::id()
        ));
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let bridge = SshStdioBridge::start(
            "example".to_string(),
            remote_bridge_command(&remote_herdr, "default", None).unwrap(),
            socket.clone(),
            None,
        )
        .expect("start bridge listener");

        let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, BRIDGE_SOCKET_PERMISSION_MODE);

        drop(bridge);
        let _ = std::fs::remove_file(socket);
    }

    #[cfg(unix)]
    #[test]
    fn accepted_bridge_stream_is_reset_to_blocking() {
        use std::os::fd::AsRawFd as _;

        fn is_nonblocking(stream: &crate::ipc::LocalStream) -> bool {
            let fd = match stream {
                crate::ipc::LocalStream::UdSocket(stream) => stream.inner().as_raw_fd(),
            };
            // SAFETY: F_GETFL only reads flags from the live descriptor owned by `stream`.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            assert!(flags >= 0, "fcntl(F_GETFL): {}", io::Error::last_os_error());
            flags & libc::O_NONBLOCK != 0
        }

        let socket = std::env::temp_dir().join(format!(
            "herdr-bridge-blocking-test-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = crate::ipc::bind_private_local_listener(&socket).expect("bind listener");
        let client = crate::ipc::connect_local_stream(&socket).expect("connect client");
        let mut server = listener.accept().expect("accept client");

        crate::ipc::set_local_stream_polling(&mut server, true)
            .expect("force the macOS accepted-stream state");
        assert!(is_nonblocking(&server));
        let server = prepare_remote_bridge_stream(server).expect("prepare bridge stream");
        assert!(!is_nonblocking(&server));

        drop(server);
        drop(client);
        drop(listener);
        let _ = std::fs::remove_file(socket);
    }

    #[cfg(windows)]
    #[test]
    fn windows_bridge_drop_while_waiting_for_client_is_bounded() {
        let socket = local_forward_socket_path("drop-test", "default");
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let bridge = SshStdioBridge::start(
            "example".to_string(),
            remote_bridge_command(&remote_herdr, "default", None).unwrap(),
            socket.clone(),
            None,
        )
        .expect("start bridge listener");
        let started = Instant::now();

        drop(bridge);

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!socket.exists());
    }

    #[cfg(unix)]
    #[test]
    fn managed_ssh_config_includes_user_config_then_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let managed_config = write_managed_ssh_config().expect("write managed config");
        let path = managed_config.options.config_path.clone();
        let control_path = managed_config
            .options
            .control_path
            .clone()
            .expect("Unix managed config has a control path");
        let contents = std::fs::read_to_string(&path).expect("read keepalive config");

        // herdr's fallback transport settings are present...
        assert!(
            contents.contains("Host *"),
            "config should add a Host * fallback block: {contents}"
        );
        assert!(
            contents.contains("ServerAliveInterval 15"),
            "config should set the keepalive interval: {contents}"
        );
        assert!(
            contents.contains("ServerAliveCountMax 4"),
            "config should set the keepalive count: {contents}"
        );
        assert!(!contents.contains("ControlMaster"));
        assert!(!contents.contains("ControlPersist"));
        assert!(!contents.contains("ControlPath"));
        // ...and any user config is Included (quoted) BEFORE it so
        // first-value-wins keeps the user's own settings.
        if let Some(home) = std::env::var_os("HOME") {
            let user_config = PathBuf::from(home).join(".ssh").join("config");
            if user_config.is_file() {
                let include = format!(
                    "Include {}",
                    ssh_config_quote(&user_config.to_string_lossy())
                );
                let include_at = contents.find(&include).expect("user config Included");
                let fallback_at = contents.find("Host *").expect("fallback present");
                assert!(
                    include_at < fallback_at,
                    "user config must be Included before herdr's fallback: {contents}"
                );
            }
        }

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, BRIDGE_SOCKET_PERMISSION_MODE,
            "keepalive config must be user-only"
        );
        // The config lives in a private 0700 dir, not a predictable temp path.
        let dir = path.parent().expect("config has a parent dir");
        let dir_mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "ssh config dir must be user-only");
        assert!(
            fits_unix_socket_path(&control_path),
            "control socket path must fit portable Unix socket limits"
        );

        drop(managed_config);
    }

    #[test]
    fn ssh_config_quote_wraps_path_with_spaces() {
        assert_eq!(
            ssh_config_quote("/home/a b/.ssh/config"),
            "\"/home/a b/.ssh/config\""
        );
    }

    #[cfg(unix)]
    #[test]
    fn remote_ssh_command_uses_managed_config_when_present() {
        let managed_config = write_managed_ssh_config().expect("write managed config");
        let config_path = managed_config.options.config_path.clone();
        let control_path = managed_config
            .options
            .control_path
            .clone()
            .expect("Unix managed config has a control path");
        let ssh = RemoteSsh {
            target: "example".to_string(),
            managed_config: Some(managed_config),
            interactive_progress: false,
        };

        let command = ssh.command();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "-F".to_string(),
                config_path.to_string_lossy().into_owned(),
                "-S".to_string(),
                control_path.to_string_lossy().into_owned(),
                "-o".to_string(),
                "ControlMaster=auto".to_string(),
                "-o".to_string(),
                "ControlPersist=yes".to_string(),
                "-T".to_string(),
                "example".to_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_managed_ssh_config_uses_keepalives_without_control_socket() {
        let managed_config = write_managed_ssh_config().expect("write managed config");
        let config_path = managed_config.options.config_path.clone();
        assert!(managed_config.options.control_path.is_none());
        let contents = std::fs::read_to_string(&config_path).expect("read managed config");
        assert!(contents.contains("ServerAliveInterval 15"));
        assert!(contents.contains("ServerAliveCountMax 4"));

        let ssh = RemoteSsh {
            target: "example".to_string(),
            managed_config: Some(managed_config),
            interactive_progress: false,
        };
        let args = ssh
            .command()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec![
                "-F".to_string(),
                config_path.to_string_lossy().into_owned(),
                "-T".to_string(),
                "example".to_string(),
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ssh_config_include_uses_forward_slashes() {
        assert_eq!(
            ssh_config_include_path(Path::new(r"C:\Users\A B\.ssh\config")),
            r#""C:/Users/A B/.ssh/config""#
        );
    }

    #[test]
    fn remote_ssh_command_is_plain_without_managed_config() {
        let ssh = RemoteSsh {
            target: "example".to_string(),
            managed_config: None,
            interactive_progress: false,
        };

        let command = ssh.command();
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args, vec!["-T".to_string(), "example".to_string()]);
    }

    #[test]
    fn remote_install_stream_command_avoids_shell_c_wrapper() {
        let command = remote_install_stream_command("/home/a b/.local/bin/herdr.tmp.123");

        assert_eq!(command, "tee '/home/a b/.local/bin/herdr.tmp.123'");
    }

    #[test]
    fn remote_install_prepare_and_commit_scripts_quote_paths() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let prepare = remote_install_prepare_script(&remote_herdr);

        assert!(prepare.contains("mkdir -p \"$dir\""));
        assert!(prepare.contains("printf '%s\\0%s\\0' \"$tmp\" \"$dest\""));
        assert_eq!(
            parse_remote_install_paths(b"/home/a b/herdr.tmp.42\0/home/a b/herdr\0").unwrap(),
            (
                "/home/a b/herdr.tmp.42".to_string(),
                "/home/a b/herdr".to_string()
            )
        );
        assert_eq!(
            parse_remote_install_paths(b"/home/a b\n/herdr.tmp.42\0/home/a b\n/herdr\0").unwrap(),
            (
                "/home/a b\n/herdr.tmp.42".to_string(),
                "/home/a b\n/herdr".to_string()
            )
        );
        assert_eq!(
            remote_install_commit_script("/home/a b/herdr.tmp.42", "/home/a b/herdr"),
            "set -eu\nchmod 755 '/home/a b/herdr.tmp.42'\nmv '/home/a b/herdr.tmp.42' '/home/a b/herdr'\n"
        );
    }

    #[test]
    fn extract_remote_args_removes_space_form() {
        let args = vec![
            "herdr".into(),
            "--remote".into(),
            "dev".into(),
            "--help".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr", "--help"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert_eq!(remote.keybindings, RemoteKeybindings::Local);
    }

    #[test]
    fn remote_progress_is_interactive_and_never_json() {
        assert!(remote_progress_enabled(false, true, true));
        assert!(!remote_progress_enabled(true, true, true));
        assert!(!remote_progress_enabled(false, false, true));
        assert!(!remote_progress_enabled(false, true, false));
    }

    #[test]
    fn extract_remote_args_removes_equals_form() {
        let args = vec!["herdr".into(), "--remote=user@host".into()];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "user@host");
        assert_eq!(remote.keybindings, RemoteKeybindings::Local);
    }

    #[test]
    fn extract_remote_args_accepts_remote_keybindings_server() {
        let args = vec![
            "herdr".into(),
            "--remote".into(),
            "dev".into(),
            "--remote-keybindings=server".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert_eq!(remote.keybindings, RemoteKeybindings::Server);
    }

    #[test]
    fn extract_remote_args_accepts_remote_keybindings_space_form() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote-keybindings".into(),
            "server".into(),
        ];
        let (cleaned, remote) = extract_remote_args(&args).unwrap();
        assert_eq!(cleaned, vec!["herdr"]);
        assert_eq!(remote.unwrap().keybindings, RemoteKeybindings::Server);
    }

    #[test]
    fn extract_remote_args_accepts_explicit_handoff() {
        let args = vec!["herdr".into(), "--remote=dev".into(), "--handoff".into()];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert_eq!(remote.target, "dev");
        assert!(remote.live_handoff);
    }

    #[test]
    fn extract_remote_args_accepts_explicit_provision_contract() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--provision".into(),
            "--yes".into(),
            "--json".into(),
        ];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, vec!["herdr"]);
        let remote = remote.unwrap();
        assert!(remote.provision);
        assert!(remote.yes);
        assert!(remote.json);
        assert!(!remote.live_handoff);
    }

    #[test]
    fn extract_remote_args_preserves_child_remote_options_after_separator() {
        let args = vec![
            "herdr".into(),
            "agent".into(),
            "start".into(),
            "repro".into(),
            "--".into(),
            "child".into(),
            "--remote".into(),
            "dev".into(),
            "--remote-keybindings=server".into(),
            "--handoff".into(),
        ];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, args);
        assert!(remote.is_none());
    }

    #[test]
    fn extract_remote_args_preserves_handoff_without_remote() {
        let args = vec!["herdr".into(), "update".into(), "--handoff".into()];

        let (cleaned, remote) = extract_remote_args(&args).unwrap();

        assert_eq!(cleaned, args);
        assert!(remote.is_none());
    }

    #[test]
    fn extract_remote_args_rejects_remote_keybindings_without_remote() {
        let args = vec!["herdr".into(), "--remote-keybindings=server".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote-keybindings requires --remote");
    }

    #[test]
    fn extract_remote_args_rejects_duplicate_remote_keybindings() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote-keybindings=local".into(),
            "--remote-keybindings=server".into(),
        ];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote-keybindings can only be specified once");
    }

    #[test]
    fn extract_remote_args_requires_value() {
        let args = vec!["herdr".into(), "--remote".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "missing value for --remote");
    }

    #[test]
    fn extract_remote_args_rejects_empty_value() {
        let args = vec!["herdr".into(), "--remote=".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "missing value for --remote");
    }

    #[test]
    fn extract_remote_args_rejects_duplicate_values() {
        let args = vec![
            "herdr".into(),
            "--remote=dev".into(),
            "--remote=prod".into(),
        ];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote can only be specified once");
    }

    #[test]
    fn extract_remote_args_rejects_option_like_target() {
        let args = vec!["herdr".into(), "--remote".into(), "-oProxyCommand=x".into()];
        let err = extract_remote_args(&args).unwrap_err();
        assert_eq!(err, "--remote target must not start with '-'");
    }

    #[test]
    fn sanitize_path_component_removes_shell_sensitive_chars() {
        assert_eq!(sanitize_path_component("user@host:22"), "user-host-22");
    }

    #[test]
    fn remote_platform_maps_uname_values() {
        assert_eq!(
            RemotePlatform::from_uname("Linux", "amd64")
                .unwrap()
                .asset_key(),
            "linux-x86_64"
        );
        assert_eq!(
            RemotePlatform::from_uname("Darwin", "arm64")
                .unwrap()
                .asset_key(),
            "macos-aarch64"
        );
        assert!(RemotePlatform::from_uname("FreeBSD", "x86_64").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn reattach_command_includes_remote_and_session() {
        assert_eq!(
            reattach_command(
                "target/release/herdr",
                "user@host",
                "work",
                RemoteKeybindings::Local,
                false,
            ),
            "target/release/herdr --remote user@host --session work"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host name",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Local,
                false,
            ),
            "herdr --remote 'host name'"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Server,
                false,
            ),
            "herdr --remote host --remote-keybindings server"
        );
        assert_eq!(
            reattach_command(
                "herdr",
                "host",
                crate::session::DEFAULT_SESSION_NAME,
                RemoteKeybindings::Local,
                true,
            ),
            "herdr --remote host --handoff"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_reattach_command_uses_current_executable() {
        let executable = std::env::current_exe().expect("current test executable");
        assert_eq!(
            reattach_command(
                r"C:\Program Files\Herdr\herdr.exe",
                "host'name",
                "work'name",
                RemoteKeybindings::Local,
                false,
            ),
            format!(
                "& '{}' --remote 'host''name' --session 'work''name'",
                executable.display().to_string().replace('\'', "''")
            )
        );
    }

    #[test]
    fn remote_bridge_command_uses_installed_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        assert_eq!(
            remote_bridge_command(&remote_herdr, crate::session::DEFAULT_SESSION_NAME, None)
                .unwrap(),
            "exec \"$HOME/.local/bin/herdr\" remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_uses_path_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "/usr/bin/herdr\n")
            .expect("path binary");

        assert_eq!(
            remote_bridge_command(&remote_herdr, crate::session::DEFAULT_SESSION_NAME, None)
                .unwrap(),
            "exec /usr/bin/herdr remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_quotes_discovered_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/herdr bin/herdr\n")
                .expect("path binary");

        assert_eq!(
            remote_bridge_command(&remote_herdr, crate::session::DEFAULT_SESSION_NAME, None)
                .unwrap(),
            "exec '/opt/herdr bin/herdr' remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_uses_macos_path_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "macos",
            arch: "aarch64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/homebrew/bin/herdr\n")
                .expect("path binary");

        assert_eq!(
            remote_bridge_command(&remote_herdr, crate::session::DEFAULT_SESSION_NAME, None)
                .unwrap(),
            "exec /opt/homebrew/bin/herdr remote-client-bridge"
        );
        assert_eq!(remote_herdr.platform.asset_key(), "macos-aarch64");
    }

    #[test]
    fn remote_path_discovery_reads_multiple_absolute_paths() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let candidates = remote_herdrs_from_path_discovery(
            &remote_herdr,
            "/usr/bin/herdr\nbin/herdr\n /opt/herdr bin/herdr\n",
        );

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].shell_path, "/usr/bin/herdr");
        assert_eq!(candidates[1].shell_path, "'/opt/herdr bin/herdr'");
    }

    #[test]
    fn remote_path_discovery_ignores_mise_shims() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let candidates = remote_herdrs_from_path_discovery(
            &remote_herdr,
            "/home/can/.local/share/mise/shims/herdr\n/home/can/.local/share/mise/installs/herdr/0.7.1/bin/herdr\n",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].shell_path,
            "/home/can/.local/share/mise/installs/herdr/0.7.1/bin/herdr"
        );
    }

    #[test]
    fn known_remote_binary_candidate_script_includes_mise_and_nix_paths() {
        let script = known_remote_binary_candidate_script(&RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });

        assert!(script.contains("emit \"$home/.local/bin/herdr\""));
        assert!(!script.contains("mise/shims/herdr"));
        assert!(script.contains(&format!("version={}", shell_quote(&current_version()))));
        assert!(
            script.contains("emit \"$home/.local/share/mise/installs/herdr/$version/bin/herdr\"")
        );
        assert!(script.contains("emit \"$home/.local/share/mise/installs/herdr/$version/herdr\""));
        assert!(script.contains(
            "emit \"$home/.local/share/mise/installs/github-ogulcancelik-herdr/$version/herdr\""
        ));
        assert!(script.contains("emit \"$home/.nix-profile/bin/herdr\""));
        assert!(script.contains("emit \"/etc/profiles/per-user/$user/bin/herdr\""));
        assert!(script.contains("emit \"/run/current-system/sw/bin/herdr\""));
        assert!(script.contains("emit \"/home/linuxbrew/.linuxbrew/bin/herdr\""));
        assert!(!script.contains("emit \"/opt/homebrew/bin/herdr\""));
    }

    #[test]
    fn known_remote_binary_candidate_script_includes_macos_homebrew_paths() {
        let script = known_remote_binary_candidate_script(&RemotePlatform {
            os: "macos",
            arch: "aarch64",
        });

        assert!(script.contains("emit \"/opt/homebrew/bin/herdr\""));
        assert!(script.contains("emit \"/usr/local/bin/herdr\""));
        assert!(!script.contains("emit \"/home/linuxbrew/.linuxbrew/bin/herdr\""));
    }

    #[test]
    fn remote_path_discovery_quotes_single_quotes_in_discovered_binary() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr =
            remote_herdr_from_path_discovery(&remote_herdr, "/opt/herdr's/bin/herdr\n")
                .expect("path binary");

        assert_eq!(
            remote_bridge_command(&remote_herdr, crate::session::DEFAULT_SESSION_NAME, None)
                .unwrap(),
            "exec '/opt/herdr'\\''s/bin/herdr' remote-client-bridge"
        );
    }

    #[test]
    fn remote_path_discovery_ignores_relative_paths() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "bin/herdr\n");

        assert!(remote_herdr.is_none());
    }

    #[test]
    fn remote_path_discovery_ignores_empty_output() {
        let remote_herdr = RemoteHerdr::for_platform(RemotePlatform {
            os: "linux",
            arch: "x86_64",
        });
        let remote_herdr = remote_herdr_from_path_discovery(&remote_herdr, "\n");

        assert!(remote_herdr.is_none());
    }

    #[test]
    fn remote_shell_path_warning_accepts_managed_install() {
        assert!(remote_shell_resolves_managed_install(
            "/home/can/.local/bin/herdr\n"
        ));
        assert!(remote_shell_resolves_managed_install(
            "/Users/can/.local/bin/herdr\n"
        ));
        assert!(!remote_shell_resolves_managed_install(
            "/usr/local/bin/herdr\n"
        ));
        assert!(!remote_shell_resolves_managed_install(""));
    }

    #[test]
    fn parse_client_status_json_reads_protocol() {
        assert_eq!(
            parse_client_status_json(r#"{"version":"x","protocol":8,"binary":"/bin/herdr"}"#)
                .map(|status| status.protocol),
            Some(8)
        );
        assert!(parse_client_status_json(r#"{"protocol":"unknown"}"#).is_none());
    }

    #[test]
    fn parse_remote_server_status_json_reads_running_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"running","running":true,"version":"0.6.0","protocol":8,"capabilities":{"live_handoff":true,"detached_server_daemon":true}}"#
            )
            .unwrap(),
            RemoteServerStatus::Running {
                version: Some("0.6.0".into()),
                protocol: Some(8),
                binary: None,
                live_handoff: true,
                detached_server_daemon: true
            }
        );
    }

    #[test]
    fn parse_remote_server_status_json_treats_missing_capability_as_old_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"running","running":true,"version":"0.6.0","protocol":8}"#
            )
            .unwrap(),
            RemoteServerStatus::Running {
                version: Some("0.6.0".into()),
                protocol: Some(8),
                binary: None,
                live_handoff: false,
                detached_server_daemon: false
            }
        );
    }

    #[test]
    fn parse_remote_server_status_json_reads_stopped_server() {
        assert_eq!(
            parse_remote_server_status_json(
                r#"{"status":"not_running","running":false,"version":null,"protocol":null}"#
            )
            .unwrap(),
            RemoteServerStatus::NotRunning
        );
    }

    #[test]
    fn remote_update_manifest_uses_root_assets_for_latest_version() {
        let manifest: RemoteUpdateManifest = serde_json::from_str(
            r#"{
                "version": "1.2.3",
                "assets": {
                    "linux-x86_64": "https://example.com/latest"
                },
                "sha256": {
                    "linux-x86_64": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                },
                "releases": {
                    "1.2.3": {
                        "assets": {
                            "linux-x86_64": "https://example.com/archive"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let release = manifest.release_for_version("1.2.3").unwrap();
        assert_eq!(
            release.assets.get("linux-x86_64").map(RemoteAssetRef::url),
            Some("https://example.com/latest")
        );
        assert_eq!(
            release.sha256.get("linux-x86_64").map(String::as_str),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn remote_update_manifest_reads_archived_release_assets() {
        let manifest: RemoteUpdateManifest = serde_json::from_str(
            r#"{
                "version": "1.2.4",
                "assets": {
                    "linux-x86_64": "https://example.com/latest"
                },
                "releases": {
                    "1.2.3": {
                        "notes": "ignored",
                        "assets": {
                            "linux-x86_64": "https://example.com/archive"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            manifest
                .release_for_version("1.2.3")
                .and_then(|release| release.assets.get("linux-x86_64"))
                .map(RemoteAssetRef::url),
            Some("https://example.com/archive")
        );
    }

    #[test]
    fn remote_update_manifest_uses_archived_release_protocol() {
        let manifest: RemoteUpdateManifest = serde_json::from_str(
            r#"{
                "version": "1.2.4",
                "protocol": 42,
                "assets": {
                    "linux-x86_64": "https://example.com/latest"
                },
                "releases": {
                    "1.2.3": {
                        "notes": "ignored",
                        "protocol": 41,
                        "assets": {
                            "linux-x86_64": "https://example.com/archive"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            manifest
                .release_for_version("1.2.3")
                .and_then(|release| release.protocol),
            Some(41)
        );
    }

    #[test]
    fn remote_update_manifest_does_not_inherit_latest_protocol_for_archived_assets() {
        let manifest: RemoteUpdateManifest = serde_json::from_str(
            r#"{
                "version": "1.2.4",
                "protocol": 42,
                "assets": {
                    "linux-x86_64": "https://example.com/latest"
                },
                "releases": {
                    "1.2.3": {
                        "notes": "ignored",
                        "assets": {
                            "linux-x86_64": "https://example.com/archive"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            manifest
                .release_for_version("1.2.3")
                .and_then(|release| release.protocol),
            None
        );
    }

    #[test]
    fn remote_preview_manifest_falls_back_to_archived_exact_build_assets() {
        let mut manifest: RemotePreviewManifest = serde_json::from_str(
            r#"{
                "prerelease": false,
                "build_id": "2026-06-06-new",
                "protocol": 12,
                "assets": {
                    "linux-x86_64": {
                        "url": "https://example.com/new",
                        "sha256": "new"
                    }
                },
                "builds": {
                    "2026-06-02-old": {
                        "protocol": 11,
                        "assets": {
                            "linux-x86_64": {
                                "url": "https://example.com/old",
                                "sha256": "old"
                            }
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let (protocol, assets) =
            preview_assets_for_build(&manifest, "2026-06-02-old").expect("archived build");
        let asset = assets.get("linux-x86_64").expect("asset");
        assert_eq!(protocol, 11);
        assert_eq!(asset.url(), "https://example.com/old");
        assert_eq!(asset.sha256(), Some("old"));

        manifest.prerelease = true;
        assert!(preview_assets_for_build(&manifest, "2026-06-02-old").is_err());
    }

    #[test]
    fn remote_server_restart_reason_requires_stop_for_protocol_mismatch() {
        assert_eq!(
            remote_server_restart_reason(Some(&current_version()), Some(0), true, false),
            Some(RemoteServerRestartReason::ProtocolMismatch)
        );
    }

    #[test]
    fn remote_server_restart_reason_allows_unchanged_compatible_server() {
        assert_eq!(
            remote_server_restart_reason(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                true,
                false
            ),
            None
        );
    }

    #[test]
    fn remote_server_restart_reason_requires_restart_for_old_daemon() {
        assert_eq!(
            remote_server_restart_reason(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                false,
                false
            ),
            Some(RemoteServerRestartReason::DaemonDetachMissing)
        );
    }

    #[test]
    fn remote_server_restart_reason_requires_restart_after_helper_update() {
        assert_eq!(
            remote_server_restart_reason(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                true,
                true
            ),
            Some(RemoteServerRestartReason::BinaryUpdated)
        );
    }

    #[test]
    fn remote_server_restart_reason_offers_restart_for_version_mismatch() {
        assert_eq!(
            remote_server_restart_reason(Some("0.0.0"), Some(CURRENT_PROTOCOL), true, false),
            Some(RemoteServerRestartReason::VersionMismatch)
        );
        assert_eq!(
            remote_server_restart_reason(None, Some(CURRENT_PROTOCOL), true, false),
            Some(RemoteServerRestartReason::VersionMismatch)
        );
    }

    #[test]
    fn remote_server_restart_reason_allows_current_server() {
        assert_eq!(
            remote_server_restart_reason(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                true,
                false
            ),
            None
        );
    }

    #[test]
    fn remote_provision_action_uses_start_reload_or_restart_from_owned_state() {
        assert_eq!(
            remote_provision_server_action(&RemoteServerStatus::NotRunning, false, false),
            RemoteProvisionServerAction::Start
        );
        let matching = RemoteServerStatus::Running {
            version: Some(current_version()),
            protocol: Some(CURRENT_PROTOCOL),
            binary: Some(r"C:\Users\Can\.herdr\remote\herdr.exe".into()),
            live_handoff: false,
            detached_server_daemon: true,
        };
        assert_eq!(
            remote_provision_server_action(&matching, false, true),
            RemoteProvisionServerAction::Reload
        );
        assert_eq!(
            remote_provision_server_action(&matching, true, true),
            RemoteProvisionServerAction::Restart
        );
        assert_eq!(
            remote_provision_server_action(&matching, false, false),
            RemoteProvisionServerAction::Restart
        );
    }

    #[test]
    fn remote_install_plan_keeps_compatible_running_server() {
        assert_eq!(
            remote_install_running_server_plan(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                true,
                false,
                false,
                false
            ),
            RemoteInstallRunningServerPlan::KeepRunning
        );
    }

    #[test]
    fn remote_install_plan_requires_stop_for_old_daemon() {
        assert_eq!(
            remote_install_running_server_plan(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                false,
                true,
                false,
                false
            ),
            RemoteInstallRunningServerPlan::StopRequired(
                RemoteServerRestartReason::DaemonDetachMissing
            )
        );
    }

    #[test]
    fn remote_install_plan_requires_stop_after_helper_update() {
        assert_eq!(
            remote_install_running_server_plan(
                Some(&current_version()),
                Some(CURRENT_PROTOCOL),
                true,
                true,
                false,
                false
            ),
            RemoteInstallRunningServerPlan::StopRequired(RemoteServerRestartReason::BinaryUpdated)
        );
    }

    #[test]
    fn remote_install_plan_requires_stop_for_incompatible_running_server() {
        assert_eq!(
            remote_install_running_server_plan(
                Some("0.0.0"),
                Some(CURRENT_PROTOCOL),
                true,
                true,
                false,
                false
            ),
            RemoteInstallRunningServerPlan::StopRequired(
                RemoteServerRestartReason::VersionMismatch
            )
        );
    }

    #[test]
    fn remote_install_plan_uses_live_handoff_for_incompatible_running_server() {
        assert_eq!(
            remote_install_running_server_plan(
                Some("0.0.0"),
                Some(CURRENT_PROTOCOL),
                true,
                true,
                true,
                true
            ),
            RemoteInstallRunningServerPlan::LiveHandoff
        );
    }

    #[test]
    fn install_source_description_uses_override_binary() {
        let platform = RemotePlatform {
            os: "linux",
            arch: "aarch64",
        };
        assert_eq!(
            install_source_description_for(&platform, Some(Path::new("/tmp/herdr-aarch64")), false),
            "HERDR_REMOTE_BINARY (/tmp/herdr-aarch64)"
        );
    }

    #[test]
    fn install_source_description_uses_local_binary_when_allowed() {
        let platform = RemotePlatform::local();

        assert_eq!(
            install_source_description_for(&platform, None, true),
            "the current local herdr binary"
        );
    }

    #[test]
    fn install_source_description_uses_release_asset_when_local_binary_cannot_seed_remote() {
        let platform = RemotePlatform::local();

        assert_eq!(
            install_source_description_for(&platform, None, false),
            format!(
                "the {} {} asset for {}",
                current_version(),
                current_channel(),
                platform.asset_key()
            )
        );
    }

    #[test]
    fn resolve_install_source_uses_override_binary_without_temporary_cleanup() {
        let platform = RemotePlatform {
            os: "linux",
            arch: "aarch64",
        };
        let source = resolve_install_source(&platform, Some(PathBuf::from("/tmp/herdr-aarch64")))
            .expect("override source");
        assert_eq!(source.path, PathBuf::from("/tmp/herdr-aarch64"));
        assert!(source.temporary_dir.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn windows_local_forward_endpoint_uses_private_state_dir() {
        let path = local_forward_socket_path("user@example.com", "work");
        assert!(path.starts_with(crate::platform::remote_private_temp_base()));
        assert!(path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("herdr-r-")));
    }

    #[cfg(unix)]
    fn remote_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[cfg(unix)]
    fn socket_path_byte_len(path: &Path) -> usize {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().len()
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_uses_readable_name_when_it_fits() {
        let _guard = remote_env_lock().lock().unwrap();
        // Short target + session leave plenty of room — keep the human-
        // readable form so the socket path stays grep-friendly.
        let path = local_forward_socket_path("dev", "default");
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        assert!(
            filename.starts_with("herdr-remote-"),
            "expected readable name, got {filename}"
        );
        assert!(filename.contains("-dev-default."), "got {filename}");
        assert!(
            fits_unix_socket_path(&path),
            "socket path too long: {} ({} bytes)",
            path.display(),
            socket_path_byte_len(&path)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_fits_in_sun_path() {
        let _guard = remote_env_lock().lock().unwrap();
        // Worst case for the readable form: macOS-style 49-char TMPDIR +
        // max-length sanitized components. Should fall back to the hashed
        // short name, which fits under TMPDIR.
        let target = "longish-host.example.com";
        let session = "a-fairly-long-session-name-here";
        let path = local_forward_socket_path(target, session);
        assert!(
            fits_unix_socket_path(&path),
            "socket path too long for sun_path: {} ({} bytes)",
            path.display(),
            socket_path_byte_len(&path)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_forward_socket_path_falls_back_to_tmp_when_dir_is_long() {
        let _guard = remote_env_lock().lock().unwrap();
        // Force a TMPDIR long enough that even the hashed short name cannot
        // fit inside it. The fallback should drop to /tmp.
        let prior = std::env::var_os("TMPDIR");
        let long_dir = std::env::temp_dir().join("a".repeat(80));
        let _ = fs::create_dir_all(&long_dir);
        std::env::set_var("TMPDIR", &long_dir);

        let path = local_forward_socket_path("longish-host.example.com", "default");
        let fits = fits_unix_socket_path(&path);
        let parent = path.parent().map(Path::to_path_buf);
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        match prior {
            Some(v) => std::env::set_var("TMPDIR", v),
            None => std::env::remove_var("TMPDIR"),
        }
        let _ = fs::remove_dir_all(&long_dir);

        assert!(fits, "fallback path still overflows: {}", path.display());
        assert_eq!(parent.as_deref(), Some(Path::new("/tmp")));
        assert!(
            filename.starts_with("herdr-r-"),
            "expected hashed fallback, got {filename}"
        );
    }

    #[test]
    fn install_source_cleanup_removes_temporary_directory() {
        let dir = std::env::temp_dir().join(format!(
            "herdr-install-source-cleanup-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).expect("create temp dir");
        let path = dir.join("herdr.tmp");
        fs::write(&path, b"test").expect("write temp file");

        InstallSource::temporary(path, dir.clone()).cleanup();

        assert!(!dir.exists());
    }
}
