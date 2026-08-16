# herdr-win repository overlay

The global OpenCode working agreement owns reusable personal workflow. This file
owns only herdr-win repository invariants and must not restate or silently
override generic delivery procedure. Project-specific commands select exact
owners and unsafe boundaries while the global interactive ordering remains
authoritative. Do not copy global workflow or create project `.opencode/`
configuration.

## Canonical owners and precedence

- Code and tests own detailed implementation behavior.
- `PRODUCT.md` owns stable user-visible fork behavior, terminology, supported
  choices, and acceptance outcomes.
- `ARCHITECTURE.md` owns stable technical boundaries, source/replay topology,
  Windows distribution design, state ownership, and verification architecture.
- `CONTRIBUTING.md` owns change classification, mailbox maintenance, replay,
  verification, documentation projection, commit, and upstream-engagement
  procedure.
- `BACKLOG.md` owns only current-user-selected future product outcomes.
- `AGENT_IMPROVEMENTS.md` owns evidence-backed herdr-win-specific workflow,
  tooling, test, and skill improvement proposals.

After system and current-user instructions, local precedence is this file,
`PRODUCT.md`, `ARCHITECTURE.md`, `CONTRIBUTING.md`, then `BACKLOG.md`.
Improvement proposals are not active rules until accepted into an owning file.

Route each durable decision to exactly one owner above. Cross-project OpenCode
behavior belongs in the global configuration repository. Do not invent another
active memory owner.

## Identity and source invariants

- Official upstream: `herdrdev/herdr`.
- This repository: `hdosys/herdr-win`, an unofficial Windows-focused
  distribution fork.
- Release source is replayed upstream source plus the maintained delta, not an
  independently developed product line.
- Runtime identity remains `herdr`: package, executable, command, config, state,
  sockets, and protocol. Fork identity belongs only in repository/release/update
  presentation.
- Keep the delta small, explicit, replayable, and upstreamable.
- Choose changed behavior against recorded upstream and the simplest long-lived
  design. Keep one implementation path for each current contract.
- `patches/delta/` owns maintained product behavior; `series` owns order and
  `BASE` records the exact commit behind the latest non-draft, non-prerelease
  upstream stable release selected during the last explicit manual refresh. It
  never tracks upstream `master` or a preview tag.
- Never fetch, clone, query, download, check out, replay, test, or otherwise obtain
  anything from official upstream `herdrdev/herdr` unless the user explicitly
  requests that exact upstream operation in the current task. This prohibition also
  covers changing `BASE` or refreshing maintained mailboxes; use only already-local
  objects and the commit recorded in `BASE`, and report a blocked gate instead of
  reaching upstream. When the user explicitly requests an upstream refresh, query
  and fetch the latest stable release, peel its exact commit, and update `BASE` only
  after the complete queue is replayed, reviewed, and verified there.
  Synchronizing this fork's configured `origin` for normal collaboration and
  delivery remains allowed because it is not official upstream. A manual
  `workflow_dispatch` release uses recorded `BASE`.
- `patches/upstream/` is a frozen patch archive. Never regenerate, rename, or
  delete it; external links may depend on exact paths.
- `scripts/test_delta_patches.py` and `scripts/test_upstream_patches.py` own queue
  control invariants.
- `.github/workflows/ci.yml` owns cheap PR/manual replay validation.
- `.github/workflows/release.yml` owns the manually dispatched candidate build and
  exact-artifact promotion path. It has no scheduled trigger. A build requires an
  explicit herdr-win CalVer and always replays recorded `BASE`; promotion requires
  the successful build run ID and never rebuilds or repackages its retained
  candidates.
- `website/preview.json` is generated channel state; the release workflow's
  promotion operation is its only writer.
- `src/distribution.rs` in replayed source owns the fork channel and source URLs.
- Root `README.md` and `docs/next/README.md` are one mirrored public-documentation
  projection of relevant `PRODUCT.md` behavior.

## Repository and session boundaries

- Treat this directory as the root and `master` as the control branch; stop for
  direction on another branch.
- Preserve recovery stashes and unrelated shared-worktree changes.
- Never commit generated replay/build evidence, logs, binaries, credentials,
  private data, or temporary worktrees.
- Final local ZIP, setup, and checksum artifacts always go under the workspace's
  ignored `target/<target-triple>/release/` directory. Temporary directories are
  intermediates only; never report an external temporary path as the primary
  user-testable artifact.

## Product and implementation constraints

- Preserve the user-visible promises in `PRODUCT.md` and technical boundaries in
  `ARCHITECTURE.md`; do not hide a changed product or architecture decision inside
  a mailbox refresh or procedural edit.
- The Windows installer accepts only the current managed install layout and exact
  product-owned roots. Reject every other root with the documented uninstall-first
  action. Preserve current managed locked-binary safety through immutable runtimes,
  per-build leases, pending activation, and process-safe uninstall.
- Status inspection is pure and never starts or retries work.
- Keyboard-first terminal interaction remains complete end to end.
- Windows PTY integration owns process-tree cleanup, handle inheritance, resize,
  and byte-stream behavior explicitly. Compile Windows-only code only for Windows.
- Do not add `unsafe` unless unavoidable at a reviewed FFI boundary.
- Avoid production `unwrap`/`expect`; propagate contextual errors.
- Use `tracing` instead of production `eprintln!`/`dbg!`.
- Keep async cancellation-safe and avoid holding non-async locks across `.await`.
- Keep `#[allow(...)]` narrow, local, and justified.
