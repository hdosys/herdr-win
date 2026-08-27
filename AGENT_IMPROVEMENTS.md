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
- Status values: proposed, accepted, blocked, declined, done.

## Proposals

- **Status: done. Scope Git trust inside the delta-worktree helper.** Every helper
  Git command now trusts only the resolved control checkout and selected worktree
  through command-scoped `safe.directory` values. Evidence: materialization had
  failed on a Sandbox-owned linked checkout until the same bounded values were
  supplied manually; the focused command-construction test now owns that contract.
  Owner: `scripts/delta_workflow.py`, its focused tests, and `CONTRIBUTING.md`.

- **Status: done. Compile each mailbox prefix during stable-base refreshes.** The
  refresh-only `compile-prefixes` command replays and compiles every ordered prefix
  with all detected logical processors, disabled incremental state, and one shared
  temporary Cargo target.
  Evidence: the v0.8.2 review found cross-mailbox owner leaks that a complete-queue
  compile hid. Moving the leaked deferred auto-start call from 0003 to its 0009
  owner made the queue compile prefix by prefix; after appending the metadata owner,
  all ten prefixes passed in 416.595 seconds. Disabling incremental output also
  removed a measured 5.4 GiB disposable-cache failure. Owner:
  `scripts/delta_workflow.py`, its focused tests, and the refresh procedure in
  `CONTRIBUTING.md`.

- **Status: done. Render-check changed README Mermaid diagrams before commit.** The
  staged hook now renders the exact mirrored block and rejects invalid source or an
  extreme aspect ratio without retaining generated output. Evidence: the revised
  compact diagram rendered at 686.328 by 518 pixels with aspect ratio 1.325, and
  the focused parser, mirror, renderer, and staged-change tests passed. Owner:
  `.githooks/pre-commit`, `scripts/readme_mermaid_check.py`, and its focused tests.

- **Status: done. Provision the Mermaid renderer in the maintained Sandbox
  tool stack.** A one-label README diagram change spent 64.401 seconds acquiring
  Mermaid CLI through `npx`, then 10.352 seconds removing its task-owned cache.
  Reusing installed Edge with the browser download disabled rendered successfully
  at 686.328 by 542 pixels. Provision or cache the supported CLI and configure
  `MERMAID_CLI` to reuse installed Edge. The maintained provisioner now resolves the
  latest stable CLI, suppresses its browser download, and writes one Edge-backed
  command into the Sandbox environment. The idempotent path rendered the README at
  686.328 by 542 pixels in 3.935 seconds. Owner: `.herdr-sandbox/provision.ps1`,
  `scripts/provision_mermaid_renderer.ps1`, and `CONTRIBUTING.md`.

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

- **Status: declined. Reuse validated binaries for package-excluded candidate
  changes.** The current candidate identity intentionally changes for every tracked
  diff and untracked source input, and that identity owns the managed runtime path.
  A package-exclusion classifier would introduce a second source truth and permit a
  runtime to claim provenance from a snapshot it did not compile. The 140.782-second
  rebuild did not justify weakening that fail-closed contract; normal-test retiering
  removes the demonstrated serial test compile without changing identity ownership.

- **Status: done. Make one development stack the only local user installer
  owner.** Product sessions default to the shared `candidate/development`
  worktree. Topic worktrees exist only for concrete internal isolation, default to
  build-ID-isolated outputs, and must be integrated and removed by their creator.
  Only the exact development branch writes the fixed setup, so every reported
  installer is the cumulative current state. User acceptance promotes that complete
  tree rather than individual topics. Owner:
  `scripts/local_windows_installer.py`, its focused tests, `AGENTS.md`, and the
  development procedure in `CONTRIBUTING.md`.

- **Status: done. Share the release-profile Cargo cache for release-boundary
  checks.** `local_windows_installer.py candidate --release-test-filter` runs one
  exact release-profile test with the packaged binaries' target, build identity,
  and detected logical-processor job count. Evidence: the implemented path passed
  its focused release test in 186.361 seconds, then reused that target for the
  packaged-binary build in 0.826 seconds. Owner: the script, its focused tests, and
  the Candidate procedure in `CONTRIBUTING.md`.

- **Status: done. Retier optimization-insensitive unit tests out of the local
  installer release-build path.** `--test-filter` now uses the existing normal
  `just test-one` gate, while explicit `--release-test-filter` retains the optimized
  boundary path. Evidence: the real cumulative candidate ran one exact normal test
  in 65.849 seconds, reused its validated release binaries in 0.982 seconds, and
  produced the setup in 27.243 seconds with 16 jobs. Owner:
  `scripts/local_windows_installer.py`, its focused tests, and the Candidate
  procedure in `CONTRIBUTING.md`.

- **Status: done. Make `just test-one` select Windows-valid Rust targets.**
  The Windows recipe now runs through native PowerShell and constrains nextest to
  the `herdr` binary while the existing non-Windows path remains unchanged.
  Evidence: the focused AutoStart filter passed all three tests and the remote
  server detach filter passed its native Windows test through the shared recipe.
  Owner: `justfile`.

- **Status: done. Give public preview-manifest propagation a sufficient bounded
  release-ready gate.** The exact raw branch URL now receives 18 bounded attempts
  instead of 12 before publication is declared stale. Evidence: the prior URL
  remained stale for the complete 110-second window and exposed the correct build
  shortly afterward; the extended workflow passed focused inventory and Actionlint
  checks. Owner: `.github/workflows/release.yml`.

- **Status: done. Document one clean sparse checkout for WinGet PR updates.** The
  contribution procedure now starts sparse mode in the initial single-branch clone,
  selects only the package cone, and requires a clean status before edits. Evidence:
  the exact path produced the clean PR head in 84.050 seconds and selected the cone
  in 3.143 seconds, replacing a full checkout that exceeded 600 seconds. Owner:
  `CONTRIBUTING.md`.

- **Status: done. Make Windows worktree removal terminal before dropping its
  recovery metadata.** The Windows lifecycle now waits for the pane process and
  ConPTY master to exit, and fails before Git mutation if bounded shutdown does not
  complete. Evidence: a real Windows Terminal client created and removed a
  non-forced worktree in 5.460 seconds, leaving neither its directory nor Git
  registration; cumulative installer build `9eb521456ac0.1f6cc92d2972` validated
  and packaged. Owner: the Windows `herdr worktree remove` lifecycle.

- **Status: done. Reuse an exact current installer bundle before compiling a
  build-only candidate.** `candidate` now derives source identity first and, when
  no focused test is requested, validates and packages an existing exact bundle
  through the shared `build` path before creating a Cargo target. Evidence: the
  real cumulative candidate reused build `9eb521456ac0.6747139eceac` and produced
  the validated setup in 29.415 seconds, including 27.094 seconds of packaging,
  instead of repeating 331.237 seconds of test and release compilation. Owner:
  `scripts/local_windows_installer.py`, its focused tests, and the Candidate
  procedure in `CONTRIBUTING.md`.

- **Status: done. Retier non-Rust focused checks before candidate
  packaging.** Let the Candidate procedure run an exact repository-owned Bun or
  Python behavior check in the same stop-on-failure command chain, then invoke
  `candidate` without a Rust `--test-filter`; reserve that option for Rust-owned
  behavior. Evidence: the OpenCode asset suite passed 36 tests in 0.434 seconds,
  while the exact Rust embedding filter consumed 118.444 seconds before a separate
  134.220-second release compile, making the validated candidate take 281.669
  seconds. Earlier, the nearest Rust embedding filter spent 38.361 seconds before
  failing on an unrelated existing Copilot assertion. This removes false blockers
  and one unnecessary test compile without adding a generic command runner. The
  procedure now runs the exact Bun or Python owner first and invokes Candidate
  without a Rust filter in the same stop-on-failure sequence. Owner: the Candidate
  procedure in `CONTRIBUTING.md`.

- **Status: done. Add one native Windows interactive-server launch probe.**
  Exercise the built product from an actual `cmd.exe` parent through the existing
  Task Scheduler launch owner so Windows hidden `=X:` drive state is present, then
  assert bootstrap consumption, exact session and user ownership, process cleanup,
  and zero task residue. Evidence: two installers passed probes whose parent lacked
  the real `cmd.exe` environment and then failed the configured SSH path. The exact
  `cmd.exe` to Task Scheduler launch passed in 3.253 seconds after the root fix. A
  reusable gate would expose the cross-session failure before installer handoff
  without reconstructing one-off test code. Candidate now validates the built bundle,
  places one real `cmd.exe` parent in a kill-on-close job, and requires bootstrap
  consumption, exact user and session ownership, process shutdown, and zero task
  residue before packaging. The Windows PowerShell 5.1 path passed in 3.883 seconds.
  Owner: `scripts/windows_interactive_server_launch_probe.ps1`, its native job helper,
  and the Candidate procedure in `CONTRIBUTING.md`.

- **Status: done. Propagate Windows `test-one` failures.** Make the Windows
  PowerShell recipe return Cargo Nextest's exit status to Just. Evidence: an
  intentionally failing focused test printed Nextest's failure summary, but
  `just test-one interactive_server` returned status 0; invoking the same Cargo
  Nextest path directly returned the failure correctly. The recipe now returns
  `$LASTEXITCODE`; direct and wrapped no-match runs both returned status 4. This
  prevents false-green focused gates. Owner: `justfile`.

- **Status: done. Gate changed integration assets on one migration-version
  advance.** Extend the existing delta or Candidate source check to compare each
  changed managed integration asset with the accepted queue baseline, then require
  a higher embedded marker and matching Rust constant before packaging. Evidence:
  three OpenCode layout commits changed the managed plugin while both old and new
  bytes still claimed version 12, so the installed stale plugin was reported as
  current. Version 13 immediately made the real installed copy detectable and
  replaceable. Candidate now compares embedded assets with the accepted replay tree,
  requires exactly one marker advance, and checks the matching Rust constant before
  bundle reuse or compilation. The focused control suite passed 36 tests and the
  live candidate comparison reported `changed=opencode`. Owner:
  `scripts/delta_workflow.py`, `scripts/local_windows_installer.py`, and their focused
  tests, without a second integration-version registry.

- **Status: proposed. Share the Candidate normal-test Cargo target with the
  pre-push focused Rust gate.** Expose one repository-owned command that runs the
  required pre-push `just test-one` boundary against Candidate's stable Cargo
  target, so Candidate can rerun the same test after push without recompiling its
  binary. Evidence: the pre-push four-test gate compiled in 30.430 seconds, then
  Candidate compiled the same Windows test binary in a separate target for 46.750
  seconds before its 6.804-second test. Reusing that cache should remove roughly 45
  seconds without skipping either gate. Owner: `scripts/local_windows_installer.py`,
  `justfile`, and the Candidate procedure in `CONTRIBUTING.md`.

- **Status: blocked. Exercise restored OpenCode roots in the native split
  probe.** Start the existing adaptive-split acceptance from a persisted OpenCode
  session after a Herdr restart, then create several direct children concurrently
  and assert the native panes. Evidence: the fresh managed-launch contract and all
  32 sequential asset tests first passed while a restored process exposed no
  attachable endpoint. After that fix, seven simultaneous child events all read the
  pre-split layout and repeatedly divided the Main pane because no prior child pane
  ID had been recorded. The exact check requires stopping and reattaching a live
  Herdr/OpenCode session plus provider-authenticated concurrent direct-child creation.
  The repository has no credential-free native child trigger, and a synthetic event
  driver would duplicate the Bun harness without proving restore or native pane
  placement. Resume only in a user-authorized provider session; the existing Bun suite
  remains the deterministic split-logic owner. Owner: the OpenCode native Candidate
  acceptance boundary, reusing its current split probe rather than a second harness.
