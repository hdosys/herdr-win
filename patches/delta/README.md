# herdr-win maintained delta

This is the canonical product delta applied by the manually dispatched herdr-win
release workflow on top of the reviewed [`herdrdev/herdr`](https://github.com/herdrdev/herdr)
stable-release commit recorded in `BASE`.

The queue intentionally contains a few coarse, logical feature patches rather
than one monolith or a patch for every development commit:

The current queue targets Herdr v0.9.0 and retains these logical slots:

- 0001: client terminal appearance, negotiated cursor color, and Windows VTI input.
- 0003: Windows SSH adapter, compatible attach, exact provisioning, and bounded desktop launch.
- 0004: managed distribution, installer lifecycle, and fork update handling.
- 0005: OpenCode lifecycle, pane-local selection, and direct-child layouts.
- 0006: shared runtime `curl` policy.
- 0008: selected-path Git trust and bounded Windows worktree terminal shutdown.
- 0009: persistent-session Agent auto-start and shared shell-native launch.
- 0010: transient foreground takeover recovery.
- 0011: 64-token metadata capacity.
- 0012: client-local completion controls and cancellation suppression.
- 0013: repeated terminal-history rows.
- 0015: muted-label contrast in client presentation.
- 0016: section-aware integration settings hints.

Slots 0007, 0014, 0017, and 0018 are absent because v0.9.0 already owns the
equivalent native-path docs assertion, plugin-root resolution, Devin configuration,
and Windows environment validation. Partial upstream adoption does not retire a
remaining responsibility.

When a feature evolves, refresh its existing mailbox in place. Add a new patch
only when the change has a genuinely independent owner, verification plan, and
upstream integration path. This keeps replay conflicts localized without
turning the queue into task history.

## Files

- `BASE` records the exact commit behind the latest non-draft, non-prerelease
  upstream stable release selected during the latest explicit manual refresh.
- `series` is the only release application order.
- `*.patch` files are full-index, binary-safe `git format-patch` mailboxes.

Repository branding, GitHub Actions, and release orchestration are control-plane
files and do not belong in this product patch queue.

## Refreshing the queue

Run this procedure only for a current user-authorized official-upstream refresh. Ordinary fork work must use the commit already recorded in `BASE` without querying or fetching newer upstream source.

1. Query the official latest stable release, fetch and peel its `v<version>` tag,
   verify it is neither draft nor prerelease, and start a clean branch at that
   exact commit.
2. Apply `series` in order with `git am --3way`.
3. Resolve upstream drift in the patch that owns the behavior.
4. Keep one reviewed commit per logical patch, preserving its metadata and stable
   filename. Do not flatten merged development history into a single mailbox.
5. Use `delta_workflow.py refresh` as documented in `CONTRIBUTING.md`. It stages
   full-index binary mailboxes privately and proves the accepted tree before
   replacing the queue and `BASE`. Every ordered prefix must compile before control
   publication; evidence for unchanged staged prefix trees remains valid.

Validate the control-plane inventory with:

```powershell
python -m unittest scripts.test_delta_patches scripts.test_upstream_patches
```

Release replay never resolves conflicts automatically. A conflict means the
owning patch must be refreshed and reviewed.
