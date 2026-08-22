# herdr-win

**Develop on Windows from Linux, macOS, or Windows with a cross-platform [Herdr](https://github.com/herdrdev/herdr) distribution that makes Windows a first-class client and server.**

[![Patch replay](https://github.com/hdosys/herdr-win/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/hdosys/herdr-win/actions/workflows/ci.yml) [![Candidate build](https://github.com/hdosys/herdr-win/actions/workflows/release.yml/badge.svg?branch=master)](https://github.com/hdosys/herdr-win/actions/workflows/release.yml) [![Rust 1.96.1](https://img.shields.io/badge/Rust-1.96.1-000000?logo=rust&logoColor=white)](https://github.com/hdosys/herdr-win/blob/master/rust-toolchain.toml) [![Built with Herdr Sandbox](https://img.shields.io/badge/built%20with-Herdr%20Sandbox-0078D4?logo=windows11&logoColor=white)](https://github.com/hdosys/herdr-sandbox) [![License: Apache 2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](https://github.com/hdosys/herdr-win/blob/master/LICENSE)

Despite its historical name, `herdr-win` is not Windows-only. It is an unofficial, upstream-first cross-platform distribution focused on Windows feature parity. Its defining use case is running a full Herdr server on a Windows workstation or VM while controlling it from the normal Herdr client on Linux, macOS, or Windows.

Every release candidate builds matching Windows, Linux, and macOS binaries from one exact reviewed Herdr release and the same ordered patch queue. That keeps clients, servers, and provisioned remote runtimes on one compatible protocol while preserving the normal `herdr` command and workflow.

> [!IMPORTANT]
> GitHub's **ahead/behind** banner compares commit ancestry, not release-source freshness. This repository's `master` is a control branch for the patch queue and release automation, not a mirror of upstream `master`. Each build starts from the reviewed stable commit in [`BASE`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/BASE) and applies [`series`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/series). Because a refresh updates those owners without merging or rebasing upstream `master`, the displayed "behind" count can remain unchanged. GitHub's **Sync fork** action is not this project's refresh mechanism.

[Demo](#see-it-in-action) · [Why it exists](#why-it-exists) · [What it adds](#what-differs-from-upstream) · [Install and cross-platform use](#install-and-cross-platform-use) · [Patch flow](#how-the-patch-queue-works) · [Upstream review](#for-upstream-maintainers) · [Maintaining](#maintaining-the-project) · [Herdr Sandbox](#sister-project-herdr-sandbox)

## See it in action

https://github.com/user-attachments/assets/b6c02367-683b-4a1f-94e6-b662149d89d9

Detach from a Windows-hosted Herdr session, reconnect from another terminal, and continue the same OpenCode session without RDP.

> [!NOTE]
> This README describes the maintained queue on `master`; use a tagged release's changelog as the exact contract for currently downloadable binaries. Upstream Herdr owns the general CLI, configuration, integrations, and product documentation. This repository owns its Windows-focused delta and cross-platform release. Reproduce general issues with upstream Herdr before reporting them here.

## Why it exists

The reviewed upstream release provides Herdr's cross-platform core. herdr-win supplies the remaining Windows behavior needed for mixed-platform development without treating Windows as a client-only or second-class target:

- **A terminal that feels native:** without explicit theme configuration, Herdr follows the host's light or dark appearance while preserving cursor colors and Windows input through ConPTY.
- **Windows as a Herdr server:** Windows, Linux, and macOS clients can attach to or provision a Windows workstation or VM over SSH with an exact runtime, protocol check, and named-session lifecycle.
- **One release across mixed environments:** matching Windows, Linux, and macOS assets let Herdr provision supported remote endpoints from the same protocol-compatible build instead of relying on manual binary copying or independently released versions.
- **The upstream image bridge stays intact:** Herdr v0.8.2 stages Windows clipboard images and supported image-file drops on the remote host and pastes their usable path. This fork no longer patches that shared transport.
- **Updates that respect running work:** per-user setup uses verified immutable runtimes, lets active sessions finish, and activates one coherent replacement afterward without a permanent background service.
- **Agent status that reflects reality:** OpenCode retries stay active instead of surfacing premature failures, while terminal errors remain visible.

The patch queue is how those outcomes stay maintainable, not the reason users should care. Every retained behavior is tied to an exact upstream stable commit and one responsibility-owned mailbox. Candidates are replayed, tested at their native boundaries, packaged as real artifacts, and promoted without rebuilding. When equivalent behavior ships upstream, it is deleted here. That keeps releases reproducible, the engineering reviewable, and the fork small enough to upstream rather than becoming a second Herdr product.

## What differs from upstream

The table is intentionally capability-level. The patch files contain the exact implementation and tests.

| Area | Status | What this repository contributes |
| --- | --- | --- |
| Native ConPTY foundation | ✅ **Upstreamed in Herdr v0.6.9** | Herdr v0.8.0 added the modern app-local ConPTY packaging that herdr-win now reuses instead of carrying a duplicate foundation. |
| Terminal fidelity | **Maintained here** · [`0001`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0001-windows-terminal-appearance.patch) | Follows host light/dark appearance by default and preserves cursor, rendering, and VTI input behavior in local and attached Windows sessions. |
| Windows SSH target support | **Maintained here** · [#2329](https://github.com/herdrdev/herdr/pull/2329) · [`0003`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0003-windows-remote-attach.patch) | Adds Windows x86_64/ARM64 host detection, one exact provisioned payload, named-session lifecycle, and fail-closed detached launch into the SSH user's active desktop session. Herdr v0.8.2 owns Windows clients attaching to Linux/macOS and the shared image bridge. |
| Managed Windows snapshots | **Maintained here** · [`0004`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0004-windows-managed-distribution.patch) | Provides per-user setup, portable archives, immutable runtime activation, update ownership, and process-safe uninstall from one verified candidate. |
| OpenCode lifecycle reporting | **Maintained here** · [`0005`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0005-opencode-retry-notifications.patch) | Keeps active retries active and exposes only actionable terminal failures. |
| Runtime downloads | **Maintained here** · [`0006`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0006-harden-curl-transfers.patch) | Makes runtime downloads independent of user `curl` configuration and bounds them to TLS 1.2+ HTTPS with limited redirects. |
| Cross-platform docs checks | **Maintained here** · [#3041](https://github.com/herdrdev/herdr/issues/3041) · [`0007`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0007-docs-parity-native-paths.patch) | Keeps the upstream documentation-parity unittest valid with native paths on Windows and POSIX systems. |
| Mounted worktree lifecycle | **Maintained here** · [#3044](https://github.com/herdrdev/herdr/issues/3044) · [`0008`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0008-worktree-scoped-git-trust.patch) | Scopes Git ownership trust to the exact repository and checkout selected by a Herdr worktree operation, without persistent or wildcard configuration. |
| New-tab Agent start | **Maintained here** · [#321](https://github.com/herdrdev/herdr/issues/321) · [`0009`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/0009-session-agent-autostart.patch) | Optionally starts one configured Agent in the root pane of each genuinely new persistent-session tab without competing with saved-session Agent resume. |

Upstream PR #2329 ships in Herdr v0.8.2. The refreshed mailbox `0003` therefore contains only Windows target-host detection, provisioning, activation, and exact interactive-session launch. Shared client attach, image transport, and SSH bridge behavior now come directly from upstream. The original Windows-host work builds on [nsxdavid's `feat/windows-remote-attach` branch](https://github.com/nsxdavid/herdr/tree/feat/windows-remote-attach).

## Sister project: Herdr Sandbox

herdr-win is developed and validated with [**Herdr Sandbox**](https://github.com/hdosys/herdr-sandbox), a disposable native Windows development environment for coding agents. It provides the clean Windows toolchains and realistic native boundary used to build and test this fork; it is a sister project, not a runtime dependency.

## How the patch queue works

```mermaid
flowchart LR
    subgraph S["Source"]
        direction TB
        U["Upstream Herdr<br/>v0.8.2"] --> B["BASE<br/>9eb521456ac0"]
    end

    subgraph Q["patches/delta/series"]
        direction TB
        P1["0001<br/>Terminal fidelity"] --> P3["0003<br/>Windows SSH target"]
        P3 --> P4["0004<br/>Windows distribution"]
        P4 --> P5["0005<br/>OpenCode lifecycle"]
        P5 --> P6["0006<br/>Hardened downloads"]
        P6 --> P7["0007<br/>Portable docs check"]
        P7 --> P8["0008<br/>Scoped Git trust"]
        P8 --> P9["0009<br/>Fresh-session Agent"]
    end

    subgraph V["Validated distribution"]
        direction TB
        R["Fresh replay"] --> G["Native + cross-platform<br/>gates"]
        G --> A["Windows setup + ZIP<br/>Linux/macOS binaries + digests"]
    end

    B --> P1
    P9 --> R
```

[`patches/delta/BASE`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/BASE) records the exact upstream stable commit. [`series`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/series) is the only application order. Each patch is a full-index, binary-safe mailbox with one logical responsibility.

An upstream refresh is deliberate: select the latest stable release, replay the complete queue, remove behavior upstream now owns, regenerate changed mailboxes, and verify a fresh replay. `BASE` never follows upstream `master` automatically.

## Install and cross-platform use

Each release is one cross-platform snapshot for Windows x86_64, Linux amd64/arm64, and macOS amd64/arm64. Windows additionally ships the managed per-user setup and portable ZIP; those executables require no separately installed Microsoft Visual C++ Redistributable. Every matching binary can run as a client or remote endpoint on the same wire protocol.

This supports mixed development environments directly: run the managed client and server on Windows, or use a matching Linux or macOS client to control a Herdr server on a Windows workstation or VM. The Linux and macOS builds can also run as remote endpoints when a Windows client connects in the other direction.

Every supported client can attach to an x86_64 or ARM64 Windows SSH host. Herdr uses an exact matching `herdr.exe` from the SSH user's `PATH` or the stable `%USERPROFILE%\.herdr\remote\herdr.exe` payload. Provisioning validates the complete digest-bearing portable ZIP before stopping a server, then requires the exclusive lease, replaces the one stable payload, and verifies the exact binary, version, and protocol. There is no rollback, legacy discovery, migration, compatibility layout, or extra `bin` level. Local Windows development builds use the same portable layout and are rejected without their ConPTY bundle. The host's OpenSSH default shell must be `cmd.exe` or PowerShell 7 (`pwsh.exe`). The server starts in exactly one active desktop session owned by the SSH user and remains independent of the initiating SSH channel; absent or ambiguous sessions fail closed. Windows remote hosts do not support live handoff or OpenSSH control-socket reuse.

For explicit unattended deployment and activation, validate the remote configuration and provision the matching binary with:

```powershell
herdr --remote workbox --provision --yes --json
```

Provision starts a missing server, reloads configuration when the running binary already matches, and saves then restarts only when a different binary must be activated.

### WinGet (recommended)

```powershell
winget install hdosys.herdr-win
```

Update with:

```powershell
winget upgrade hdosys.herdr-win
```

Open a new terminal and run:

```powershell
herdr --version
herdr
```

The per-user installer requires no administrator access and installs Herdr's
canonical agent skill while preserving customized copies. Published builds
identify themselves as `herdr-win <CalVer> (Herdr <upstream-version>)`.

### Direct setup alternative

<p align="center">
  <img src="https://raw.githubusercontent.com/hdosys/herdr-win/master/docs/assets/herdr-win-setup-welcome.png" alt="Herdr Win setup welcome page">
</p>

If WinGet is unavailable, download
`herdr-win_v<version>_windows_amd64_setup.exe` from the latest
[Herdr Win release](https://github.com/hdosys/herdr-win/releases/latest), verify
its GitHub SHA-256 digest, and run it. Direct installations update with
`herdr update`; running sessions stay on their current runtime until the
replacement can activate safely.

### Uninstall

Uninstall from **Windows Settings → Apps → Installed apps**. Uninstall first stops running managed Herdr sessions through their graceful server API. If a session cannot stop within the bounded deadline, the managed installation is preserved and the required action is reported. Settings are preserved unless you explicitly choose to remove them. Uninstall never force-terminates sessions or removes unowned or unsafe content.

### Portable Windows ZIP

The release also includes `herdr-win_v<version>_windows_amd64.zip`. Extract the complete archive into one directory and run `herdr.exe`.

> [!WARNING]
> The Herdr executable and setup are currently unsigned, so Windows may show a SmartScreen warning. Download release artifacts only from this repository.

### Linux and macOS clients and endpoints

Each release includes raw `linux_amd64`, `linux_arm64`, `macos_amd64`, and `macos_arm64` executables. They are supported unbundled clients and remote endpoints built from the same retained candidate; unlike Windows, they are not managed installers.

Use matching binaries from the same herdr-win release on every endpoint. Independently released official builds are not guaranteed to use the same wire protocol.

For general commands, configuration, and agent integrations, use the [official Herdr documentation](https://herdr.dev/docs/).

## For upstream maintainers

Thank you for the native Windows foundation and the remote-client work merged in [#2329](https://github.com/herdrdev/herdr/pull/2329). herdr-win exists to make remaining Windows behavior easy to inspect, test, and remove from this fork as equivalent support lands upstream. The Windows remote-host follow-up is tracked in [Discussion #2409](https://github.com/herdrdev/herdr/discussions/2409).

The files listed in `patches/delta/series` are the complete maintained product delta. Each is self-contained, so its behavior does not need to be reconstructed from this fork's development history.

1. Start at the exact commit in [`BASE`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/BASE).
2. Apply [`series`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/series) in order with `git am --3way`.
3. Review each mailbox as one responsibility-oriented change with its implementation, tests, and documentation.
4. Follow [`CONTRIBUTING.md`](https://github.com/hdosys/herdr-win/blob/master/CONTRIBUTING.md) to reproduce the replay and verification gates.

The mailboxes are offered as focused evidence and acceptance tests, not an all-or-nothing merge request. Upstream can split or reimplement them along its own ownership boundaries; once equivalent behavior ships, herdr-win removes it. Fork branding, release workflows, and publication state stay outside the product queue.

## Maintaining the project

Refresh and release are intentionally separate manual operations:

- **Refresh:** select and review a stable upstream release, then replay and minimize the queue.
- **Build:** replay recorded `BASE`, run the complete gates, and retain one candidate with provenance and checksums.
- **Promote:** publish those exact retained bytes without rebuilding or repackaging them.

Ordinary pushes do not publish binaries.

| Need | Canonical owner |
| --- | --- |
| User-visible fork behavior | [`PRODUCT.md`](https://github.com/hdosys/herdr-win/blob/master/PRODUCT.md) |
| Technical boundaries | [`ARCHITECTURE.md`](https://github.com/hdosys/herdr-win/blob/master/ARCHITECTURE.md) |
| Patch ownership and refresh policy | [`patches/delta/README.md`](https://github.com/hdosys/herdr-win/blob/master/patches/delta/README.md) |
| Replay, verification, and release procedure | [`CONTRIBUTING.md`](https://github.com/hdosys/herdr-win/blob/master/CONTRIBUTING.md) |
| Open work | [`BACKLOG.md`](https://github.com/hdosys/herdr-win/blob/master/BACKLOG.md) |

## Issues and contributions

- Use [upstream Herdr](https://github.com/herdrdev/herdr) for general product behavior that reproduces with an official upstream build.
- Use [herdr-win issues](https://github.com/hdosys/herdr-win/issues) for this distribution's artifacts, update feed, workflows, or maintained patches.
- Read [`CONTRIBUTING.md`](https://github.com/hdosys/herdr-win/blob/master/CONTRIBUTING.md) before changing the queue or release automation.

## Credits and license

Herdr is created and maintained upstream by [Can Çelik](https://github.com/ogulcancelik). herdr-win is distributed under the [Apache License 2.0](https://github.com/hdosys/herdr-win/blob/master/LICENSE).
