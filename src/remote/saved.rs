use std::io;
use std::path::PathBuf;

use super::attach::{find_installed_remote_herdr, RemoteSsh, SshStdioBridge};

pub(crate) struct SavedSshBridge {
    _bridge: SshStdioBridge,
}

pub(crate) struct SavedSshStream {
    pub(crate) stream: crate::ipc::LocalStream,
    pub(crate) bridge: SavedSshBridge,
}

pub(crate) fn connect_saved_ssh(
    profile_id: &str,
    target: &str,
    session: &str,
) -> io::Result<SavedSshStream> {
    validate_profile_path_id(profile_id)?;
    crate::session::validate_name(session)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let ssh = RemoteSsh::new_noninteractive(target.to_owned(), session.to_owned());
    let remote_herdr = find_installed_remote_herdr(&ssh)?;
    let path = saved_bridge_path(profile_id);
    let bridge = SshStdioBridge::start(
        target.to_owned(),
        super::attach::remote_bridge_command(&remote_herdr, session, true)?,
        path.clone(),
        ssh.options(),
        true,
    )?;
    let stream = crate::ipc::connect_local_stream(&path)?;
    Ok(SavedSshStream {
        stream,
        bridge: SavedSshBridge { _bridge: bridge },
    })
}

pub(crate) fn saved_ssh_bootstrap_command(target: &str, session: &str) -> String {
    format!(
        "herdr --remote {} --session {}",
        super::shell_quote(target),
        super::shell_quote(session)
    )
}

pub(crate) fn saved_ssh_failure_needs_attention(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::InvalidInput
            | io::ErrorKind::InvalidData
            | io::ErrorKind::NotFound
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::Unsupported
    ) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    [
        "permission denied",
        "host key verification failed",
        "remote host identification has changed",
        "no matching host key",
        "unsupported remote platform",
        "not ready",
        "install or update",
        "protocol",
        "handshake",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn saved_bridge_path(profile_id: &str) -> PathBuf {
    let pid = std::process::id();
    let readable = format!("herdr-ssh-{pid}-{profile_id}.sock");
    let short = format!("herdr-s-{pid}-{}.sock", &profile_id[..16]);
    crate::platform::remote_bridge_endpoint_path(&readable, &short)
}

fn validate_profile_path_id(profile_id: &str) -> io::Result<()> {
    if profile_id.len() == 32
        && profile_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SSH endpoint profile id",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_paths_use_profile_identity_not_target_or_session() {
        let first = saved_bridge_path("0123456789abcdef0123456789abcdef");
        let second = saved_bridge_path("fedcba9876543210fedcba9876543210");
        assert_ne!(first, second);
        assert!(!first.to_string_lossy().contains("example.com"));
        assert!(!first.to_string_lossy().contains("default"));
    }

    #[test]
    fn bootstrap_command_preserves_the_explicit_remote_session() {
        assert_eq!(
            saved_ssh_bootstrap_command("build host", "agent work"),
            "herdr --remote 'build host' --session 'agent work'"
        );
    }

    #[test]
    fn prompt_and_compatibility_failures_require_attention() {
        for message in [
            "Permission denied (publickey)",
            "Host key verification failed",
            "matching Herdr is not ready; install or update",
            "handshake rejected",
        ] {
            assert!(saved_ssh_failure_needs_attention(&io::Error::other(
                message
            )));
        }
        assert!(!saved_ssh_failure_needs_attention(&io::Error::new(
            io::ErrorKind::TimedOut,
            "network timed out"
        )));
    }
}
