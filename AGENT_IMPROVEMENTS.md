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

- **Status: done. Scope Git trust inside the delta-worktree helper.** Every helper
  Git command now trusts only the resolved control checkout and selected worktree
  through command-scoped `safe.directory` values. Evidence: materialization had
  failed on a Sandbox-owned linked checkout until the same bounded values were
  supplied manually; the focused command-construction test now owns that contract.
  Owner: `scripts/delta_workflow.py`, its focused tests, and `CONTRIBUTING.md`.

- **Status: proposed. Compile each mailbox prefix during stable-base refreshes.**
  Evidence: the v0.8.2 review found mailbox 0003 referring to owners introduced by
  0004, while 0004 referred to the curl API introduced by 0006. The complete queue
  could compile while those ordering and ownership leaks remained hidden. Extend
  the existing delta-queue test owner with a refresh-only check that replays and
  compiles each prefix in series order. Expected benefit: catch mailbox coupling
  before regeneration without slowing normal iteration. Owner:
  `scripts/test_delta_patches.py` and the refresh procedure in `CONTRIBUTING.md`.

- **Status: proposed. Render-check changed README Mermaid diagrams before commit.**
  Evidence: a fully horizontal patch flow was unreadably small at README width,
  while changing every level to vertical produced a multi-screen diagram. A
  three-column render preview measured 622 by 484 pixels and exposed the balanced
  composition before push. Add one exact-block render and aspect-ratio review to
  the existing README mirror check when Mermaid source changes. Expected benefit:
  prevent visually unusable diagram iterations without adding a broad visual gate.

- **Status: proposed. Add a repository-owned local Windows input acceptance probe.**
  Evidence: a task-local probe hardcoded the development state directory and used
  a PATH-dependent sentinel that detached servers did not inherit, producing false
  negatives before direct pane readback isolated the native ConPTY encoding fault.
  A later probe spent 93 seconds rebuilding a trace client, then initially targeted
  the independent debug state directory and incurred two 10-second rendered-text
  waits before an exact socket override and client/server logs isolated the input
  path. An auto-start probe isolated state but not config, restored the shared
  session, then left its corrected task server alive through a 122-minute session
  interruption and blocked installer preflight until bounded cleanup. Proposed
  change: add one bounded real-Windows-Terminal probe that derives isolated config
  and state ownership from the tested binary, accepts an exact socket/session
  target, scopes tracing to one client role, waits on protocol state instead of
  rendered text, verifies injected input through the shared path, and always
  cleans its named process tree and session. Expected benefit: faster
  local-versus-remote attribution, reliable terminal regression evidence, and no
  leaked task server blocking packaging. Owner: a focused script and tests under
  `scripts/`, documented in `CONTRIBUTING.md`.

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

- **Status: done. Make one development stack the only local user installer
  owner.** Product sessions default to the shared `candidate/development`
  worktree. Topic worktrees exist only for concrete internal isolation, default to
  build-ID-isolated outputs, and must be integrated and removed by their creator.
  Only the exact development branch writes the fixed setup, so every reported
  installer is the cumulative current state. User acceptance promotes that complete
  tree rather than individual topics. Owner:
  `scripts/local_windows_installer.py`, its focused tests, `AGENTS.md`, and the
  development procedure in `CONTRIBUTING.md`.

- **Status: done. Share the release-profile Cargo cache between focused native
  checks and candidate installer builds.** `local_windows_installer.py candidate
  --test-filter` now runs one focused test before packaging with the same release
  profile, explicit Windows target, target directory, build identity, and detected
  logical-processor job count as the packaged binaries, and requires exactly one
  passing test before packaging. Evidence: the implemented path compiled and passed
  the focused release test in 186.361 seconds, then reused that target for the
  packaged-binary build in 0.826 seconds. The executable path removes one cold target
  graph and rejects failing or unmatched filters before packaging. Owner:
  `scripts/local_windows_installer.py`, its focused tests, and the Candidate
  procedure in `CONTRIBUTING.md`.

- **Status: proposed. Retier optimization-insensitive unit tests out of the local
  installer release-build path.** Evidence: the background-worktree-focus candidate
  spent 240.039 seconds compiling and running one exact release-profile unit test,
  then another 147.156 seconds compiling the three production binaries; the full
  candidate took 420.254 seconds. Run pure behavior filters through the existing
  normal `just test-one` iteration gate before publication, then let the candidate
  build only the packaged release binaries. Keep the release-profile filter for
  tests whose signal actually depends on optimization or a native release boundary.
  Expected benefit: remove a second serial release compilation from ordinary
  behavior-fix installer delivery while preserving one focused product check.
  Owner: `scripts/local_windows_installer.py`, `justfile`, their focused tests, and
  the Candidate procedure in `CONTRIBUTING.md`.

- **Status: done. Make `just test-one` select Windows-valid Rust targets.**
  The Windows recipe now runs through native PowerShell and constrains nextest to
  the `herdr` binary while the existing non-Windows path remains unchanged.
  Evidence: the focused AutoStart filter passed all three tests and the remote
  server detach filter passed its native Windows test through the shared recipe.
  Owner: `justfile`.

- **Status: proposed. Give public preview-manifest propagation a sufficient
  bounded release-ready gate.** Evidence: release publication committed the new
  manifest and all immutable assets were live, but the exact raw branch URL served
  the prior manifest for the complete 110-second retry window and exposed the new
  build shortly afterward. The failed-job replay then re-entered the otherwise
  successful publish job. Extend the existing post-publish readiness budget from
  measured propagation or isolate its idempotent verifier so a transient cache miss
  does not repeat publication. Expected benefit: avoid false failed releases while
  retaining exact public-feed verification. Owner: `.github/workflows/release.yml`.

- **Status: proposed. Keep successful portable release jobs free of unrelated
  runner warnings.** Evidence: every portable artifact passed, while macOS jobs
  still annotated the release for an unused untrusted preinstalled `aws/tap`, and
  the pinned cache action reported its Node.js 20 runtime being forced onto Node.js
  24. Remove only the unused tap exposure and adopt an eligible Node.js 24-native
  pinned action when available, without trusting the whole tap or weakening action
  pinning. Expected benefit: keep release annotations actionable. Owner:
  `.github/workflows/release.yml`.

- **Status: proposed. Document one clean sparse checkout for WinGet PR updates.**
  Evidence: a full `winget-pkgs` clone entered a 630,078-file checkout and hit the
  600-second limit at 68 percent; a later `--no-checkout` sparse setup represented
  omitted paths as staged deletions. Starting with `git clone --filter=blob:none
  --sparse --single-branch --branch <pr-branch>` produced the exact clean PR head in
  84.050 seconds, and selecting the package cone took 3.143 seconds. Add that exact
  path to the existing release contribution procedure before manifest edits.
  Expected benefit: remove ten-minute checkout waits and avoid dirty-index risk
  during routine external package updates. Owner: `CONTRIBUTING.md`.

- **Status: done. Make Windows worktree removal terminal before dropping its
  recovery metadata.** The Windows lifecycle now waits for the pane process and
  ConPTY master to exit, and fails before Git mutation if bounded shutdown does not
  complete. Evidence: a real Windows Terminal client created and removed a
  non-forced worktree in 5.460 seconds, leaving neither its directory nor Git
  registration; cumulative installer build `9eb521456ac0.1f6cc92d2972` validated
  and packaged. Owner: the Windows `herdr worktree remove` lifecycle.
