# Contributing to herdr-win

Thanks for helping keep Herdr useful on Windows.

herdr-win is an upstream-first distribution, not an independent product fork.
Changes should either reduce the Windows delta, keep it replayable, or improve
the small control plane that validates and publishes it.

Read `PRODUCT.md` for stable user-visible fork behavior and `ARCHITECTURE.md` for
stable technical/source boundaries. This file owns the procedure for changing
those owners and their implementation; it does not duplicate their decisions.

## Choose the right owner

Before editing, classify the change:

- **Upstream Herdr behavior:** propose it to
  [`herdrdev/herdr`](https://github.com/herdrdev/herdr) under that
  project's contribution policy. Do not use the fork to bypass upstream review.
- **Maintained Windows behavior:** implement and validate it on a topic branch or
  `candidate/development`. Update the owning logical mailbox in `patches/delta/`
  only after the current user explicitly authorizes a patch update or release.
- **Fork control plane:** edit repository branding, contributor and delta-workflow
  automation, patch inventory tests, or the two workflows directly in this
  repository.
- **Frozen patch archive:** do not refresh or rename files in
  `patches/upstream/`; existing links must remain valid.
- **Stable user-visible fork behavior:** update `PRODUCT.md` and the mirrored
  public README projection. Update an applicable logical mailbox only under the
  explicit patch authorization below.
- **Stable technical design:** update `ARCHITECTURE.md` and the owning code/tests.
  Update a mailbox only under the explicit patch authorization below.
- **Selected future product work:** use `BACKLOG.md` only after the current user
  chooses the outcome for later implementation. It never owns findings,
  verification assignments, evidence, or test reminders.
- **Repository-specific process improvements:** use `AGENT_IMPROVEMENTS.md` while
  proposed, then move accepted procedure to this file or the owning automation.

Open an issue only when a substantial change still needs product or architecture
scope alignment before implementation. A bounded change whose scope is already
accepted does not wait on issue ceremony. A useful bug report includes the
herdr-win release tag, Windows version, terminal, shell, exact reproduction,
current behavior, and expected behavior.

## Developing the maintained delta

The patch queue is the release representation, not the day-to-day editing surface.
Do not make a product-source edit only in this repository's control checkout. The
development build starts from recorded `BASE`, so every finished product
change selected for a release must eventually be represented by the canonical
queue.

Patch promotion is a hard user-authorization boundary. Ordinary development,
candidate building, installer acceptance, clean-slate work, and completion of a
topic or development commit never authorize patch generation or a write under
`patches/`. Before invoking `delta_workflow.py finalize`, `git format-patch`, or
any equivalent patch-generation path, the current user must have explicitly
requested an update, regeneration, or finalization of the maintained patches, or
creation or publication of a release. If the current request does not already
name that outcome, stop and ask the user explicitly before the first such action.
Do not carry authorization from an earlier task. Until authorized, commit and push
only the topic branch or `candidate/development`; leave `patches/delta/`, its
`series`, and `BASE` byte-identical.

Maintained product-source work uses one long-lived shared worktree on
`candidate/development`. Its local branch and remote
`origin/candidate/development` target use the same name and are the repository's
one cumulative development state. Documentation and control-plane changes stay in
the control checkout. Ordinary sessions reopen, coordinate, and work directly in
that development tree. They do not create a worktree per issue.

Integrate every completed topic into `candidate/development` immediately after its
focused check and push that cumulative branch. An explicit current-user patch or
release request gates patch-queue promotion, not routine integration of completed
development work.

Only when `candidate/development` does not yet exist locally or on `origin`, create
the development tree from the exact local commit recorded in `BASE`, replay the
queue, and publish it:

```powershell
$control = (Get-Location).Path
$base = (Get-Content -LiteralPath patches/delta/BASE -Raw).Trim()
$created = herdr worktree create --cwd $control --branch "candidate/development" --base $base --no-focus --json | ConvertFrom-Json
python scripts/delta_workflow.py materialize --worktree $created.result.worktree.path
python scripts/delta_workflow.py publish-development `
  --worktree $created.result.worktree.path
```

After that first publication, `origin/candidate/development` is the authoritative
recovery source for the cumulative line. Reopen its registered worktree, or in a
fresh checkout create the same local branch from the exact fetched remote tip.
Never reconstruct it from `BASE` while the remote branch exists.

`materialize` validates the checkout, shared repository identity, exact `BASE`,
and clean state before applying `series` once with `git am --3way`. Its Git trust is
scoped to the control checkout and selected worktree. It never queries official
upstream.

Create a topic worktree only for a concrete parallel collision or risky isolation
boundary. Base it on the exact current development commit, not `BASE`. The creating
agent owns integration, remote durability, and complete cleanup. After its focused
check, merge the finished commit into `candidate/development`, publish it through
`delta_workflow.py publish-development`, then remove the topic worktree, local
branch, and any temporary remote ref. Never ask the user to classify or clean
these internal resources.

For every interactive development update:

1. Obtain exclusive ownership for overlapping files and build resources in the
   shared development tree.
2. Make the smallest source change and run its focused check.
3. Commit every completed change to the development branch. Integrate any completed
   topic lanes, review the cumulative diff, and push the branch to
   `origin/candidate/development` through `delta_workflow.py publish-development`.
4. Build the fixed installer only from that pushed cumulative commit. Never report
   a topic artifact.
5. Tell the user only the fixed path, hash, included outcomes, result, and next
   action. Internal worktrees, refs, and integration state are not user decisions.

Both development publication and the fixed installer inspect registered linked
topic worktrees. A committed topic head that is not an ancestor of the development
head blocks the operation. Dirty uncommitted topic state remains in progress and is
not part of the reported completed superset.

Only an explicit current-user request to update, regenerate, or finalize the
maintained patches, or to create or publish a release, authorizes promotion of the
complete reported development tree. A statement that the fixed installer works
does not authorize patch generation. Authorization never promotes an individual
topic. One session owns the complete path:

1. Reinspect shared ownership, collect completed handoffs, and stop overlapping
   writes. Reuse the focused evidence while its source, inputs, and environment
   assumptions remain unchanged.
2. Regenerate every owning mailbox represented in the accepted development range.
   Use isolated linear replay worktrees internally when a mailbox finalizer needs
   one:

   ```powershell
   python scripts/delta_workflow.py finalize `
     --worktree <durable-absolute-worktree-path> `
     --mailbox <series-entry.patch> `
     --expected-tree <tested-tree-id>
   ```

   It requires a clean current WIP branch, preserves its commits, regenerates only
   the named mailbox, keeps later mailboxes byte-identical, and writes the mailbox
   only after a complete candidate replay matches the tested tree. A conflict or
   mismatch leaves the checked-in mailbox unchanged.
   For an explicitly approved new responsibility, append one higher-numbered
   mailbox from a candidate containing exactly one WIP commit over the current
   queue:

   ```powershell
   python scripts/delta_workflow.py finalize `
     --worktree <durable-absolute-worktree-path> `
     --mailbox <new-series-entry.patch> `
     --expected-tree <tested-tree-id> `
     --new-mailbox
   ```

   This mode derives commit metadata from that WIP commit, renumbers existing
   format-patch subjects, appends the series entry, and restores every delta input
   if exact replay verification fails.
3. Run the inventory tests below. A matching tree transfers the source evidence to
   the checked-in queue, so mailbox regeneration alone does not require another
   product gate.
4. Require the complete checked-in queue to reproduce the exact accepted
   development tree. Review, commit, and push `master`, refresh the development
   baseline, rebuild the cumulative installer, and clean every integrated topic
   worktree, local branch, and temporary remote ref. These are internal mechanics
   and require no further user approval.

Installer acceptance does not authorize publication. Release remains a separate
explicit user request.

There is no scheduled or nightly delta replay. Run replay and mailbox inventory
only during a patch promotion explicitly authorized under the boundary above, an
explicitly assigned release, or a separately requested read-only diagnosis. A
release and any `BASE` refresh remain blocked until the complete accepted
development tree has been promoted into the canonical queue.

Keep the queue small and responsibility-oriented rather than mirroring development
commit history. Never hand-edit a diff to force application; regenerate the owning
mailbox from its reviewed logical commit. A replay conflict or tree mismatch is a
real promotion blocker. Repeated builds, broad gates, and raw review of generated
mailbox churn are not substitutes for source review plus exact tree identity.

### Fast cumulative Windows development installer

The control checkout owns one thin local entrypoint that reuses the materialized
source packager. It adds no installer implementation. Only the exact
`candidate/development` branch writes
`target/x86_64-pc-windows-msvc/release/herdr-win_local_candidate_setup.exe`, which
is the one artifact reported for user testing. It always contains the current replay
plus every completed change integrated into the development branch. Topic branches
automatically use build-ID-isolated outputs and those paths stay internal.

Before each user handoff, collect every completed topic handoff, commit the coherent
development tree, and push `origin/candidate/development`. Candidate packaging first
compares every changed embedded integration with the accepted queue, requires one
migration-version advance, and requires its Rust constant to match. After a
Rust-owned source change, run one exact focused behavior check while building the
candidate:

```powershell
$testFilter = "<one exact test filter>"
python scripts/local_windows_installer.py candidate `
  --source-worktree <development-worktree> `
  --test-filter $testFilter
```

The command keeps one Sandbox-local Cargo target and passes the detected logical
processor count through the build. `--test-filter` runs the ordinary behavior check
through the existing `just test-one` iteration gate before compiling only the three
packaged release binaries. Use `--release-test-filter` instead only when the selected
test's signal depends on optimization or another release-profile boundary. After
validating the exact input bundle and before packaging, Candidate exercises its built
runtime from `cmd.exe` through the Windows interactive Task Scheduler server-launch
path. The probe places only that command shell in a bounded kill-on-close job, then
verifies bootstrap consumption, user and session ownership, terminal process cleanup,
and zero scheduled-task residue. It replaces the fixed setup only after the selected
test, source identity, native probe, bundle, and package checks pass. Never report an
isolated topic artifact or ask the user which topic belongs in the installer.

For a Bun- or Python-owned behavior change, run the exact repository command first
and invoke Candidate packaging without a Rust filter in the same stop-on-failure
sequence. Do not add a generic command runner to the installer entrypoint:

```powershell
Push-Location -LiteralPath <development-worktree>
try {
    bun test src/integration/assets/opencode/herdr-agent-state.test.ts
    if ($LASTEXITCODE -ne 0) { throw "OpenCode integration asset check failed" }
} finally {
    Pop-Location
}
python scripts/local_windows_installer.py candidate `
  --source-worktree <development-worktree>
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

For a build-only repeat of the exact unchanged pushed source, omit the test filter:

```powershell
python scripts/local_windows_installer.py candidate `
  --source-worktree <development-worktree>
```

The command derives the exact current build identity first. When its validated input
bundle already exists, it delegates directly to the existing `build` path before
creating a Cargo target or compiling. A missing bundle follows the ordinary candidate
compile path. Do not omit focused verification after a source behavior change; omit
the Rust filter only when an exact non-Rust owner already passed immediately before
Candidate packaging.

Prepare a persistent ignored input bundle directly only when supplying already
built runtime, launcher, helper, or staged ConPTY payloads:

```powershell
python scripts/local_windows_installer.py prepare `
  --source-worktree <materialized-source-worktree> `
  --stage-dir <validated-stage-directory> `
  --launcher-exe <herdr-launcher.exe> `
  --installer-helper-exe <herdr-installer-helper.exe>
```

The command copies only regular non-reparse inputs below
`target/x86_64-pc-windows-msvc/installer-inputs/<build-id>/`, records every file
SHA-256, and binds the bundle to the runtime and launcher identities. It never
extracts a prior setup with 7-Zip. The source ConPTY validator remains the stage
owner.

For development installer, artwork, copy, validator, or packaging-only iterations,
reuse that bundle without rebuilding Rust payloads:

```powershell
python scripts/local_windows_installer.py build `
  --source-worktree <materialized-source-worktree> `
  --input-bundle <reported-input-bundle>
```

Every invocation rechecks all bundle hashes, exact ConPTY stage contents, runtime
and launcher identity, then delegates to the materialized source's existing NSIS
packager. The exact development branch writes the setup to the one short
replaceable path
`target/x86_64-pc-windows-msvc/release/herdr-win_local_candidate_setup.exe` and
reports its new hash. A topic branch selects its isolated bundle and setup; pass
`--isolated` explicitly when forcing that behavior on the development branch. A
tiny, clearly bounded daytime packaging-only request uses
a soft goal of roughly two minutes from the user request to that installable
artifact. Request-to-artifact is the primary user-wait metric; build time,
development integration, remote backup, and promotion are separate timings. The goal
is never a deadline or reason to stop discovery, diagnosis, or a running build;
complex work has no two-minute expectation. Report the installer immediately so
user and agent testing can continue in parallel. Missing or corrupt inputs are a
clear preparation blocker, not authority to unpack an old installer or repeatedly
rebuild unchanged payloads.

After reporting the local installer and before dispatching a remote release build,
run the complete installer fault matrix against that same validated bundle:

```powershell
python scripts/local_windows_installer.py release-precheck `
  --source-worktree <materialized-source-worktree> `
  --input-bundle <reported-input-bundle>
```

This is the exact local owner for installer recovery, hard-termination, managed
skill, and pending-update acceptance. It runs through Windows PowerShell 5.1,
uses a short ignored output directory, and removes its generated fault installers
after completion. The pending-update fixture deliberately constructs Windows CRLF
checkout input before writing canonical package bytes, and holds its synthetic
runtime lease until an explicit signal releases it. Never replace either boundary
with a fixed-duration sleep. Passing evidence remains reusable while the source
worktree, bundle bytes, and relevant environment remain unchanged.

### Refreshing from official upstream

Do not fetch, merge, rebase, or advance the queue to newer
`herdrdev/herdr` source as part of an ordinary feature, fix, documentation, or
maintenance task. Refresh official upstream only when the user explicitly requests
that separate operation. For every approved refresh:

1. Query the official latest GitHub release and require it to be neither draft nor
   prerelease.
2. Fetch its exact `v<version>` tag, peel the release commit, and verify the tag
   version matches replayed Cargo package version.
3. Replay and review the complete queue on that commit, dropping upstreamed hunks
   and anything no longer required by current fork behavior from its logical owner.
4. Run `python scripts/delta_workflow.py compile-prefixes` so every ordered mailbox
   prefix compiles before the refreshed queue can hide cross-mailbox ownership.
5. Update `BASE` only after the reviewed replay succeeds, regenerate every changed
   mailbox, replay the checked-in queue again from a fresh checkout, and run all
   refresh gates.

Between explicit refreshes, `BASE` remains pinned to that reviewed stable release;
there is no scheduled upstream query, replay, build, or release. Manual candidate
build and promotion dispatches are not an upstream refresh; the build operation
must use the stable commit already recorded in `BASE`.

Repository branding, GitHub Actions, patch metadata, and release orchestration
must not be included in product mailboxes.

Patch 0004 owns the replayed managed Windows distribution. Preserve the update,
installer, exact-layout rejection, and cross-agent-skill boundaries documented in
`ARCHITECTURE.md`; changing one is an explicit architecture/product decision, not
incidental mailbox maintenance.

Every explicit release assignment and every ConPTY dependency refresh must query
the official Microsoft package and release metadata, then select the newest
non-preview `Microsoft.Windows.Console.ConPTY` package that has been published for
at least seven complete days. Keep it reproducible: pin its exact package version,
release tag, URL, package SHA-256, and extracted runtime hashes in
`packaging/windows/conpty.json`. Never use a floating version, preview package, or
runtime auto-update. If the newest eligible stable package is already pinned, do
not create version churn. A changed pin must pass the focused package/vendor checks
and the packaged native ConPTY gate before promotion.

`website/preview.json` is generated by the promotion operation after release
publication. Do not hand-edit it or include its manifest-only commit in release
source identity. Repository release immutability is a required external setting
mirrored by the `HERDR_RELEASE_IMMUTABILITY_ENABLED=true` repository variable. The
workflow must still verify the actual release is immutable, generate the manifest
with the candidate's tested replay generator, commit only `website/preview.json`,
and fail closed if `master` advances beyond the candidate's control revision.

Use the single manual **Build and promote herdr-win release** workflow in two
separate dispatches:

1. Choose `build` and supply one unused herdr-win CalVer in `YYYY.MM.DD.N` format.
   Use the intended UTC release date and increment `N` for another release that
   day. The successful run retains the complete candidate and its provenance for
   14 days but does not publish a release. Record its workflow run ID. The workflow
   derives one candidate-scoped runtime build ID from the selected control commit,
   run ID, and attempt; never assign that identity to a separately built local
   artifact.
2. Review the successful candidate run, then choose `promote` and supply that run
   ID before its artifacts expire. Promotion accepts no replacement CalVer and
   does not replay source, compile, or package; it publishes only the candidate's
   validated files as one normal, non-prerelease GitHub release and marks it Latest.
   If `master` has advanced or the candidate expired, dispatch a new build instead.

Do not reuse one CalVer for different source. Linux and macOS publish raw
`herdr-win_v<CalVer>_{linux,macos}_{amd64,arm64}` executables. Windows publishes
`herdr-win_v<CalVer>_windows_amd64.zip` and appends `_setup.exe` for setup; upstream
package versions and source/control hashes remain separate provenance. Preserve
these machine-consumed names; show the stable Herdr version beside the CalVer in
the GitHub release title, notes, and installer metadata instead of changing
updater-facing filenames.

The retained candidate compiles that CalVer into every platform binary.
`herdr --version` must be `herdr-win <CalVer> (Herdr <upstream-version>)`; Windows
setup and Installed Apps use the same CalVer as their primary display version.
Separately built local artifacts use the literal `local` identity plus their build
ID when one is available, and must never claim a release CalVer.
That CalVer is also the fork update-order key. Promotion must reject a candidate
whose CalVer is equal to or older than the published manifest, and the updater
must reject an equal or older feed CalVer regardless of build ID. Build ID remains
the immutable runtime and matching remote-asset key; protocol remains the
client/server compatibility gate. The distribution feed is fixed at compile time,
with no `herdr channel` command or `update.channel` setting.

The build fails closed on replay conflict, source drift, or a wrong installer pin.
Promotion additionally validates the selected successful workflow run and attempt,
source/control identities, expected file set, and every digest before publication;
it also fails on missing or mutable assets, a draft or prerelease classification,
or feed content that was not fetched and verified independently. Runtime builds
retain the manifest's `preview` schema token for wire-format compatibility, while
the required `prerelease: false` value gates update selection and CalVer owns fork
update order and release presentation. All builds use only fork-owned update/setup
sources.

Manual release work is ephemeral: never create or force-push an integration
branch, merge upstream into a release branch, resolve replay conflicts
automatically, or publish releases from ordinary pushes. A conflict fails closed.
On a promotion rerun for the same candidate, the existing immutable release is
canonical; validate and reuse its complete platform asset set, derive each manifest
digest independently from the downloaded canonical release, and never replace or
repoint an asset. Promotion never deletes a tag or GitHub release. Preserve any
draft or mutable release and fail before manifest publication instead of trying to
recover by removing public state.

### Preparing a WinGet manifest update

Start from the exact pull-request branch with Git's sparse mode enabled before any
checkout populates the working tree. Never convert a full clone or a `--no-checkout`
clone into this path after the index has represented omitted files:

```powershell
git clone --filter=blob:none --sparse --single-branch --branch <pr-branch> `
  https://github.com/microsoft/winget-pkgs.git <checkout>
git -C <checkout> sparse-checkout set manifests/h/hdosys/herdr-win
git -C <checkout> status --short --branch
```

The final status must be clean before editing package manifests. This procedure
only prepares an external contribution checkout; it does not authorize manifest
changes, a pull request, or release publication.

## Verification

The **Fork verification policy** in `AGENTS.md` is the admission gate for every
new or retained herdr-win check. Change evidence must identify its stable contract,
unique realistic failure, cheapest reliable layer, and observed runtime. Remove or
retier a check when those facts no longer justify its maintenance or wait cost.
Workflow review rejects software provisioned only to satisfy a test.

During explicit promotion, run the fast inventory checks from the control repository:

```powershell
python -m unittest scripts.test_delta_patches scripts.test_upstream_patches
```

Run formatting and the smallest changed-behavior test in the replayed task tree
before recording its tested tree ID. The finalizer's exact tree match transfers
that evidence to the checked-in queue without another checkout, compile, or test
pass. Do not run blanket Clippy or all Rust tests for every ordinary edit.

For Windows packaging changes, select only the focused package or vendor modules
that own the changed boundary rather than running the whole list by default:

```powershell
python -m unittest scripts.test_package_windows_conpty
python -m unittest scripts.test_vendor_libghostty_vt
python -m unittest scripts.test_vendor_portable_pty
```

After the first artifact, run Windows-target Clippy only when the exact changed
boundary requires it. Broad native and cross-platform matrices belong to an
explicit release or unattended verification assignment:

```powershell
cargo clippy --bins --locked --target x86_64-pc-windows-msvc -- -D warnings
```

An explicit release assignment uses the manually dispatched workflow's `build`
operation for the Linux/macOS target builds and machine checks as well as the
signed ConPTY package, native quiet-uninstall checks, installer-helper lifecycle
and focused fault-retry matrix, managed launcher, and system-fallback gates that
depend on GitHub's Windows runner.

Workflow changes require `actionlint` plus review of triggers, permissions,
credential persistence, immutable source identity, artifact digests, and failure
behavior. Native package or installer changes build and report the local artifact
first, then run the smallest equivalent real-platform evidence. The full release
gate remains reserved for an explicit release assignment.

Documentation, process, and canonical-owner-only changes that do not alter a
mailbox or executable workflow use inline review, `git diff --check`, README mirror
checks when applicable. Run queue inventory tests only when the change touches
`BASE`, `series`, mailbox or archive invariants, or their control scripts. Pure
documentation and process changes do not require product replay, a Rust gate, or
the native installer matrix.

## Documentation

Keep `PRODUCT.md` as the concise canonical user-visible truth and
`ARCHITECTURE.md` as the stable technical truth. Project their relevant public
facts into the fork README without turning it into internal design documentation.
Keep root `README.md` and `docs/next/README.md` byte-for-byte identical. Product
documentation carried in release source belongs in the logical mailbox that owns
the behavior. Root `CHANGELOG.md` records only herdr-win CalVer releases and
user-visible fork changes; link the official upstream Herdr changelog instead of
copying upstream release entries. Do not edit changelog, release notes, website,
or broad docs unless changed behavior requires it, and never edit generated
preview/version documentation directories. The repository pre-commit hook renders
a changed staged Mermaid block through `mmdc`, requires the mirrored block to
match, and rejects extreme aspect ratios before generated output can enter the
worktree. Maintained Sandboxes install the latest stable Mermaid CLI into their
external tool root, suppress its Chromium download, reuse installed Microsoft Edge,
and set `MERMAID_CLI` automatically. Set `MERMAID_CLI` manually only in a
nonstandard environment.

## Pull requests and commits

- Keep pull requests focused on one logical owner.
- Explain how the queue was replayed and which Windows gates ran.
- Use lowercase conventional commit subjects, without emoji or AI co-author
  lines.
- Do not commit generated artifacts, credentials, logs, or temporary replay
  checkouts.
- Do not open upstream issues or pull requests on someone else's behalf.

By contributing, you agree that your changes are licensed under the repository's
Apache License 2.0.
