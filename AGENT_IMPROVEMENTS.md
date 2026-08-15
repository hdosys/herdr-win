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
  Proposed change: add one bounded real-Windows-Terminal probe that derives state
  ownership from the tested binary, verifies injected text through `pane read`,
  reports the selected input backend, and cleans its named session. Expected
  benefit: faster local-versus-remote attribution and reliable keyboard regression
  evidence. Owner: a focused script and tests under `scripts/`, documented in
  `CONTRIBUTING.md`.

- **Status: proposed. Add one local installer artifact entrypoint.** Evidence: once
  coherent payload, launcher, and helper inputs existed, this packaging-only change
  produced a replacement setup in 20.311 seconds and 48.723 seconds from edit to
  artifact. Preparing those inputs first hit corrupt shared Cargo output and a
  corrupt registry resource across three bounded builds, then required manual
  extraction and restaging from a prior coherent setup; the first extracted path
  was also rejected because `$PLUGINSDIR` is not NSIS-safe input. Proposed change:
  add one repository-owned command that builds or accepts an already validated
  identity-matched input triplet, delegates staging and packaging to the existing
  owners, keeps reusable inputs under ignored `target/` keyed by build ID, and
  prints the packager's structured artifact result. Expected benefit: preserve the
  sub-minute packaging loop while removing repeated argument reconstruction and
  unsafe ad hoc staging. Owner: a thin `scripts/` or `justfile` entrypoint and its
  procedure in `CONTRIBUTING.md`.
