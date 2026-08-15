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
- **Maintained Windows behavior:** update the owning logical mailbox in
  `patches/delta/`.
- **Fork control plane:** edit repository branding, contributor and delta-workflow
  automation, patch inventory tests, or the two workflows directly in this
  repository.
- **Frozen patch archive:** do not refresh or rename files in
  `patches/upstream/`; existing links must remain valid.
- **Stable user-visible fork behavior:** update `PRODUCT.md`, the owning logical
  mailbox when applicable, and the mirrored public README projection.
- **Stable technical design:** update `ARCHITECTURE.md` and the owning code/tests
  or mailbox together.
- **Open product work or process improvements:** use `BACKLOG.md` or
  `AGENT_IMPROVEMENTS.md` respectively; do not leave them in task logs.

Open an issue only when a substantial change still needs product or architecture
scope alignment before implementation. A bounded change whose scope is already
accepted does not wait on issue ceremony. A useful bug report includes the
herdr-win release tag, Windows version, terminal, shell, exact reproduction,
current behavior, and expected behavior.

## Developing the maintained delta

The patch queue is the release representation, not the day-to-day editing surface.
Do not make a product-source edit only in this repository's control checkout. The
manual candidate build starts from recorded `BASE`, so every finished product
change must still be represented by the canonical queue.

A replayed task worktree is required only for maintained product-source changes,
because the patch queue is their release representation. Documentation and
control-plane changes stay in the control checkout. Use one task worktree for one
accepted logical milestone and reuse it only while that same milestone remains
active. Size alone does not justify another worktree. Independent milestones use
separate branches and worktrees because tree identity covers the complete replayed
source tree.

Create the task tree once from the exact local commit already recorded in `BASE`.
In a Herdr-managed session, the global worktree lifecycle remains authoritative:

```powershell
$control = (Get-Location).Path
$base = (Get-Content -LiteralPath patches/delta/BASE -Raw).Trim()
$created = herdr worktree create --cwd $control --branch "agent/delta-<task-slug>" --base $base --no-focus --json | ConvertFrom-Json
python scripts/delta_workflow.py materialize --worktree $created.result.worktree.path
```

Omit `--path` so Herdr selects its configured worktree root and opens the linked
checkout as a managed workspace. Outside a Herdr-managed session, the repository
helper can create the same task tree below an existing durable parent:

```powershell
python scripts/delta_workflow.py start --name <task-slug> --path <durable-absolute-new-path>
```

`materialize` validates the existing checkout, branch, clean state, shared
repository identity, and exact `BASE` before applying `series` once with
`git am --3way`. Neither path queries or fetches official upstream.

The default interactive result is a verified, remotely backed up source candidate,
not an updated mailbox. Candidate work stays within one logical mailbox owner and
one accepted milestone. During candidate iteration:

1. Edit the replayed source directly and run only the required formatter plus the
   smallest test or real boundary probe that exercises the changed behavior.
2. When the change has a local package or installer path, build and report the
   first coherent artifact at its canonical `target/` path now. Queue inventory,
   fresh replay, broad lint, and native or release matrices do not precede that
   artifact unless an exact unsafe boundary has no cheaper reliable evidence.
3. Review the ordinary source diff, commit the focused-tested source candidate,
   and record `git rev-parse 'HEAD^{tree}'`.
4. Push each coherent artifact-backed commit as a fast-forward remote backup. The
   remote candidate ref, not the worktree, is the durable owner:

   ```powershell
   git push origin "HEAD:refs/heads/candidate/<mailbox-id>/<task-slug>"
   ```

   Never force-push a candidate. A non-fast-forward failure is an ownership or
   integration blocker. Do not rely on a Sandbox or worktree surviving.
5. Report the worktree, local and remote branches, source commit and tree, owned
   files and resources, focused evidence, and artifact. Then stop. Do not regenerate
   a mailbox, modify the control checkout, push `master`, or remove the candidate
   worktree.

Mailbox promotion requires a later explicit current-user instruction that names
the candidate and owning mailbox. The original implementation request, elapsed
time, a passing candidate, a nightly process, or an agent recommendation is not
promotion approval. After approval, one session owns the complete promotion path:

1. Reinspect shared ownership, collect completed handoffs, and stop overlapping
   writes. Reuse the focused evidence while its source, inputs, and environment
   assumptions remain unchanged.
2. Run the finalizer with the recorded tree and the existing mailbox that owns the
   behavior:

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
3. Run the inventory tests below. A matching tree transfers the source evidence to
   the checked-in queue, so mailbox regeneration alone does not require another
   product gate.
4. Review the final control diff after folding. Commit and push the finished
   control milestone, then remove the task worktree through the lifecycle that
   created it only after remote durability and ownership are clear. Candidate
   branch retention or deletion is a separate explicit maintenance decision.

There is no scheduled or nightly delta replay. Run replay and mailbox inventory
only during an explicitly approved promotion, an explicitly assigned release, or
a separately requested diagnosis. A release and any `BASE` refresh remain blocked
until every selected candidate has been promoted into the canonical queue.

Keep the queue small and responsibility-oriented rather than mirroring development
commit history. Never hand-edit a diff to force application; regenerate the owning
mailbox from its reviewed logical commit. A replay conflict or tree mismatch is a
real promotion blocker. Repeated builds, broad gates, and raw review of generated
mailbox churn are not substitutes for source review plus exact tree identity.

### Fast local Windows installer candidates

The control checkout owns one thin local entrypoint that reuses the materialized
source packager. It adds no installer implementation. Prepare a persistent ignored
input bundle only when the runtime, launcher, helper, or staged ConPTY payload
changes:

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

For installer, artwork, copy, validator, or packaging-only iterations, reuse that
bundle without rebuilding Rust payloads:

```powershell
python scripts/local_windows_installer.py build `
  --source-worktree <materialized-source-worktree> `
  --input-bundle <reported-input-bundle>
```

Every invocation rechecks all bundle hashes, exact ConPTY stage contents, runtime
and launcher identity, then delegates to the materialized source's existing NSIS
packager. The setup is written to the one short replaceable path
`target/x86_64-pc-windows-msvc/release/herdr-win_local_candidate_setup.exe` and
reports its new hash. A known packaging-only edit must reach that artifact within
two minutes of the user request; candidate commit, remote backup, and promotion are
separate timings. Missing or corrupt inputs are a clear preparation blocker, not
authority to unpack an old installer or repeatedly rebuild unchanged payloads.

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
4. Update `BASE` only after the reviewed replay succeeds, regenerate every changed
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
   validated files. If `master` has advanced or the candidate expired, dispatch a
   new build instead.

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
it also fails on missing or mutable assets or feed content that was not fetched and
verified independently. Runtime builds retain the manifest's `preview` schema token
for wire-format compatibility, while CalVer owns fork update order and release
presentation; all builds use only fork-owned update/setup sources.

Manual release work is ephemeral: never create or force-push an integration
branch, merge upstream into a release branch, resolve replay conflicts
automatically, or publish releases from ordinary pushes. A conflict fails closed.
On a promotion rerun for the same candidate, the existing immutable release is
canonical; validate and reuse its complete platform asset set, derive each manifest
digest independently from the downloaded canonical release, and never replace or
repoint an asset.

## Verification

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
python -m unittest scripts.test_windows_installer
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
signed ConPTY package, enhanced-input, native quiet-uninstall checks,
installer-helper lifecycle and fault-retry matrix, managed launcher, and
system-fallback gates that depend on GitHub's Windows runner.

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
the behavior. Do not edit changelog, release notes, website, or broad docs unless
changed behavior requires it, and never edit generated preview/version
documentation directories.

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
