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

- **Status: proposed. Automate the remaining mailbox finalization mechanics.**
  Evidence: earlier milestones repeated manual `format-patch` and worktree cleanup,
  hit Windows path-length and linked-worktree trust failures, and lost time to
  unusable workspace-local Cargo intermediates. The accepted fast path now owns
  one replayed task tree and exact tree verification, but logical folding, bounded
  Cargo target selection, artifact publication, and proven-safe cleanup remain
  manual. Proposed change: after real use confirms those steps still dominate,
  extend `scripts/delta_workflow.py` with one bounded finalization operation.
  Expected benefit: remove the remaining repeated mechanics without adding another
  source authority. Owner: `scripts/delta_workflow.py`, its focused tests, and
  `CONTRIBUTING.md`.

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

- **Status: proposed. Isolate Windows PowerShell 5.1 packaging modules.**
  Evidence: the ConPTY packager failed to load `Microsoft.PowerShell.Security`
  when an inherited PowerShell 7 module path introduced duplicate type data;
  restricting the child to native Windows PowerShell module roots made the same
  package succeed. Proposed change: make the repository packaging entrypoint own
  and test its Windows PowerShell 5.1 module environment. Expected benefit:
  deterministic signature and archive validation across mixed PowerShell installs.
  Owner: `scripts/package_windows_conpty.ps1` and its focused tests.
