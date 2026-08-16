# BACKLOG.md

Current-user-selected future herdr-win product outcomes only. User-visible product
rules belong in `PRODUCT.md`; stable technical design belongs in
`ARCHITECTURE.md`; workflow/tooling/test/skill proposals belong in
`AGENT_IMPROVEMENTS.md`.

## Rules

- Keep items actionable and current.
- Remove completed or obsolete items instead of preserving history.
- Do not use this file for untriaged findings, verification assignments, evidence,
  test reminders, task logs, accepted product rules, architecture, or agent
  process notes.

## Items

### Simplify unreleased Windows remote provisioning

- Before the first Windows-host release, use one stable private payload root at
  `%USERPROFILE%\.herdr\remote\herdr.exe`. Keep ZIP transfer, digest and complete
  payload validation in temporary staging, and the exclusive runtime lease.
- Remove the extra `bin` level, rollback-directory swap, recognized-legacy-payload
  discovery and cleanup, and every migration or compatibility path. Nothing using
  the current layout has been released.
- After staging succeeds, stop the server, require the lease, remove the existing
  stable payload, promote the staged directory, then start and verify the exact
  binary, version, and protocol. A pre-publication failure leaves the old payload;
  a later failure leaves an absent or validated reprovisionable payload rather than
  restoring an older version.
- Owner: mailbox `patches/delta/0003-windows-remote-attach.patch`, with its
  `PRODUCT.md`, `ARCHITECTURE.md`, and public README projections updated together.
