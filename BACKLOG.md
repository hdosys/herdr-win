# BACKLOG.md

Open, planned, blocked, or deferred herdr-win product work only. User-visible
product rules belong in `PRODUCT.md`; stable technical design belongs in
`ARCHITECTURE.md`; workflow/tooling/test/skill proposals belong in
`AGENT_IMPROVEMENTS.md`.

## Rules

- Keep items actionable and current.
- Remove completed or obsolete items instead of preserving history.
- Include expected verification when known.
- Do not use this file for task logs, accepted product rules, architecture, or
  agent process notes.

## Items

- Bring Windows remote-host bootstrap to Unix parity before proposing mailbox
  `0003` upstream. Reuse the shared remote orchestration instead of adding a
  second Windows bootstrap path:
  1. Detect Windows hosts from every supported client OS, not only from a native
     Windows client. Probe architecture together with platform identity and reject
     every unsupported target before launch. Resolve `herdr.exe` from the SSH
     user's `PATH`, then require the same exact client version and wire protocol
     checks used for Unix candidates. Preserve non-interactive, no-mutation
     behavior. Make the encoded PowerShell launcher propagate the resolved
     executable's native exit code so discovery and bridge failures reach the
     local client truthfully. If an existing PATH binary is incompatible and the
     user declines sidecar deployment, report both identities and the exact next
     action instead of falling through to a generic handshake error.
  2. Add an explicit interactive sidecar deployment path for a missing or
     mismatched Windows host binary. Consume the existing digest-bearing
     `windows-x86_64` ZIP for the exact client build, stage and validate the
     complete portable payload because `herdr.exe` can depend on its app-local
     ConPTY sidecars, publish it atomically in a versioned user-owned sidecar
     directory, and never run the managed installer or rewrite the remote user's
     `PATH`. Suppress or redirect the normal update action for that sidecar so it
     cannot install a separate managed Windows package. Retain a payload while a
     running server may still use it and prune only exact inactive sidecars.
     Support `HERDR_REMOTE_BINARY` only through an explicit payload contract, not
     by silently copying one executable out of a packaged Windows install. Ensure
     every upstream channel that supports this path publishes the matching
     digest-bearing Windows portable asset.
  3. Run the shared running-server status and restart policy before bridge launch.
     A compatible detached server stays running. A protocol mismatch or changed
     binary requires the existing interactive stop confirmation and bounded
     shutdown check. Keep live handoff unsupported until Windows can transfer its
     PTY and process handles safely.
  4. Add a real Windows OpenSSH acceptance lane covering default and named
     sessions, missing and matching payloads, consent and non-interactive refusal,
     mismatched protocol, server persistence after SSH loss, reconnect, terminal
     resize and input, bounded bridge teardown, and clipboard-image cleanup. Test
     Windows clients against both Windows and Unix targets, plus Linux and macOS
     clients against a Windows target. Treat OpenSSH control socket reuse and
     direct terminal attach as separate platform capabilities, not blockers for
     full-workspace remote attach.
  5. After the acceptance lane passes on a fresh recorded-`BASE` replay, fold the
     result into mailbox `0003`, refresh its docs to one cross-platform bootstrap
     contract with explicit platform differences, minimize it against the first
     stable release containing upstream remote-client support, and prepare the
     upstream PR as Windows host support plus shared-orchestration refactoring.
     Include source-verified parity evidence and the Windows-to-Windows matrix in
     the PR description and continue the accepted direction in upstream
     Discussion #2409 rather than submitting the current multi-responsibility
     mailbox as-is.
- Measure and reduce warm `herdr --remote` attach latency, especially on Windows
  clients where managed SSH multiplexing is unavailable. Consolidate serial
  platform, binary, and server probes plus avoidable launcher boundaries without
  weakening host verification, authentication, protocol/version checks, or remote
  install safety. Verify warm and cold Windows-to-Windows and Windows-to-Unix
  timings before and after the change.
