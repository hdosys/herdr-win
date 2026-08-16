# AGENT_IMPROVEMENTS.md

Evidence-backed proposals for making future herdr-win agent work faster, cheaper,
safer, or more reliable.

This is not product backlog or task history. Product work belongs in `BACKLOG.md`;
user-visible behavior in `PRODUCT.md`; repository-specific technical design in
`ARCHITECTURE.md`; accepted herdr-win-specific agent rules in `AGENTS.md` or
procedure in `CONTRIBUTING.md`; cross-project workflow in the global OpenCode
configuration repository.

## Rules

- Add only concrete proposals likely to help future work.
- Keep entries short and evidence-based.
- Merge duplicates instead of appending repeats.
- Do not include secrets, credentials, private data, transient process IDs, logs,
  transcripts, generated evidence, or product feature requests.
- Status values: proposed, accepted, declined, done.

## Proposals

- **Status: proposed. Add a repository-owned local Windows input acceptance probe.**
  Evidence: a task-local probe hardcoded the development state directory and used
  a PATH-dependent sentinel that detached servers did not inherit, producing false
  negatives before direct pane readback isolated the native ConPTY encoding fault.
  A later probe spent 93 seconds rebuilding a trace client, then initially targeted
  the independent debug state directory and incurred two 10-second rendered-text
  waits before an exact socket override and client/server logs isolated the input
  path. Proposed change: add one bounded real-Windows-Terminal probe that derives
  state ownership from the tested binary, accepts an exact socket/session target,
  scopes tracing to one client role, waits on protocol state instead of rendered
  text, verifies injected input through the shared path, and cleans its named
  session. Expected benefit: faster local-versus-remote attribution and reliable
  keyboard regression evidence. Owner: a focused script and tests under `scripts/`,
  documented in `CONTRIBUTING.md`.

- **Status: done. Use one validated local installer input bundle and artifact
  entrypoint.** `scripts/local_windows_installer.py` now records exact bundle
  hashes and executable identity below ignored `target/`, then delegates repeated
  builds to the materialized source packager without Cargo or 7-Zip. Evidence: a
  cached bundle was revalidated in 1.098 seconds and produced the next atomically
  replaced setup in 23.840 seconds. Owner: the script, its focused tests, and the
  Candidate procedure in `CONTRIBUTING.md`.

- **Status: done. Use one all-core source-candidate installer command.**
  `scripts/local_windows_installer.py candidate` now derives local identity before
  compilation, overrides inherited single-job limits with every logical processor
  available to the current process, builds only the three packaged binaries, and
  always finishes at the validated installer. Evidence: the same incremental Herdr
  rebuild fell from more than 20 minutes to 99.284 seconds, followed by a 22.122
  second installer build and 125.719 seconds total. Owner: the script, its focused
  tests, and the candidate procedure in `CONTRIBUTING.md`.
