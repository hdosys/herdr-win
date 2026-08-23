# ARCHITECTURE.md

## Purpose and Authority

This file owns herdr-win's stable technical design: upstream/fork boundaries,
source and patch ownership, Windows runtime and installer topology, durable state,
release architecture, and verification lanes. `PRODUCT.md` owns user-visible
behavior; code and tests remain the detailed implementation truth.

## Source and Ownership Model

- Release source is a fresh checkout of the exact commit behind the upstream stable
  release recorded in `BASE` plus the ordered `patches/delta/series` queue. At each
  explicit manual refresh, that commit must be the latest non-draft,
  non-prerelease stable release then published by `herdrdev/herdr`; the control
  branch is not an integration product branch.
- Active control-plane checkouts, contributor and contact links, and fork-owned
  installer presentation use the current official repository, `herdrdev/herdr`.
  Imported source at recorded `BASE`, historical changelogs and version snapshots,
  vendor provenance, and the frozen patch archive preserve references from their
  recorded source boundary instead of creating a temporary fork delta.
- `BASE` is a deliberate reviewed stable-release boundary, not a moving pointer.
  Ordinary work and fork-origin synchronization never advance it. Between explicit
  user-directed refreshes, releases stay on the recorded stable source even if
  upstream moves. There is no scheduled upstream replay or release path; manual
  release dispatch always replays the recorded `BASE` and verifies its `v<Cargo
  version>` stable tag.
- Each maintained product responsibility has one logical mailbox. Evolving a
  responsibility refreshes its mailbox rather than appending development history.
  A new mailbox requires an independent owner, verification plan, and upstream
  integration path.
- Repository branding, queue controls, GitHub workflows, release metadata, and
  generated channel state are control-plane concerns and stay outside product
  mailboxes.
- `patches/upstream/` is a frozen historical archive. `patches/delta/` is the only
  active product delta; `series` is the only application order.
- Upstream owns general Herdr behavior. Fork-specific implementation must reuse
  upstream owners where practical and must not create a parallel command, config,
  protocol, state namespace, or general product implementation.

## Maintained Delta Boundaries

- Mailbox 0001 owns Windows terminal appearance, color/cursor transport, rendering,
  and Windows VTI input behavior.
- Mailbox 0001 synchronizes only the host terminal's default foreground,
  background, and cursor through OSC 10/11/12. Indexed pane colors use Herdr's
  built-in palette; truecolor remains direct. Herdr never automatically queries
  the 256 OSC 4 palette entries because terminal replies and user input share one
  uncorrelated byte stream. A burst of delayed or fragmented replies can therefore
  reach pane input, while an idle timeout cannot prove reply ownership
  (`herdrdev/herdr#2786`). Restoring host indexed-palette discovery requires a
  deterministic transport or framing boundary. Longer waits, retries, tail
  reconstruction, suppression, and other timing heuristics are not accepted paths.
- Mailbox 0003 owns only the remaining Windows SSH target boundary. Upstream Herdr
  owns shared remote orchestration, Windows clients attaching to Unix hosts, the
  named-pipe/SSH byte bridge, and bounded clipboard/file image transport. Every
  supported client probes a Windows SSH host through explicit PowerShell, accepts
  only x86_64 or ARM64, and reuses upstream exact-version, protocol, named-session,
  status, stop, and restart owners. It prefers `herdr.exe` from the SSH user's
  `PATH`, then one stable user-owned payload at
  `%USERPROFILE%\.herdr\remote\herdr.exe`.
- Published provisioning transfers the complete digest-bearing portable ZIP with
  OpenSSH. Herdr validates remote configuration plus payload digest and layout in
  transient staging before stopping a server. Activation then requires the one
  exclusive runtime lease, removes the old stable payload, promotes staging, and
  verifies the exact binary, version, and protocol after start. There is no remote
  rollback, legacy discovery, migration, compatibility layout, extra `bin` level,
  launcher, installer helper, Active/Pending pointer, or persistent version set.
  Build ID remains exact manifest and status metadata, never a directory name.
- The finite probe reads OpenSSH's `DefaultShell`. The byte bridge invokes Herdr
  directly through `cmd.exe` or PowerShell 7; Windows PowerShell 5.1 and unknown
  shells fail before bridge creation because buffered native stdout would corrupt
  the protocol. Windows OpenSSH control-socket reuse remains disabled. A matching
  API status is not attachment readiness until the binary client-protocol socket
  accepts a handshake.
  A non-package-managed local Windows x86_64 build may run on Windows ARM64 through
  x64 emulation, but it is a valid provisioning source only when its executable is
  adjacent to the exact portable license, ConPTY bundle, ownership marker, and
  notices. Herdr projects those required files into a temporary validated ZIP,
  compares the executable SHA-256 when probing the stable remote runtime, and
  rejects an incomplete local layout instead of silently selecting system ConPTY.
- Mailbox 0003 owns `--remote <target> --provision`, with `--yes` required
  for unattended mutation and optional `--json` results. Provision validates
  remote config before activation. No server means start; the same running binary
  means config reload; a different or unknown running binary means session save,
  bounded stop, and exact-binary restart. Pong and `status server` expose the
  resolved server executable for that decision. Persistent server launch resolves
  exactly one active WTS desktop session owned by the SSH user, launches through a
  transient Task Scheduler COM registration, verifies the resulting process
  session, and removes task/bootstrap state. It has no WMI, Explorer heuristic,
  session-0 fallback, persistent task, or Sandbox bridge. The bridge propagates the
  directly invoked Herdr child exit code. This path never runs the managed
  installer, changes remote `PATH`, adds compatibility layouts, or enables Windows
  live handoff or OpenSSH control-socket reuse.
- Mailbox 0004 owns deterministic ConPTY packaging, managed Windows distribution,
  fork update sources, and installer lifecycle.
- Mailbox 0005 owns OpenCode retry/error lifecycle correlation. It must preserve
  actionable terminal failures without surfacing transient errors during an active
  retry.
- Mailbox 0006 owns the shared runtime `curl` transfer policy. Runtime fetches ignore
  user `curl` configuration, pass URLs as option values, require TLS 1.2 or newer
  HTTPS for initial requests and redirects, disable URL globbing, and allow at most
  five redirects; callers retain any narrower timeout, size, and digest checks.
- Mailbox 0009 owns opt-in Agent auto-start and Windows managed-Agent command
  submission through the interactive shell. Initial workspace creation and the
  shared UI, CLI, and API tab-create path queue only each new tab's root pane while
  the existing managed Agent launch owner waits for shell readiness. A live reload
  that enables or changes the selected Agent also queues each eligible existing
  shell-only tab root once; an unchanged reload cannot duplicate pending work.
  Restore or reattach at startup, live handoff, `--no-session`, pane split, and
  existing managed-Agent terminals do not queue this path. Native Agent resume
  remains the only Agent launch path for restored Agent terminals. On Windows,
  argument-bearing PowerShell launches resolve only executable `PATHEXT`
  applications before `Start-Process`, while argument-free launches retain one
  concise native command line. The Windows-focused Rust tests and a real shell
  round trip own that boundary.

## Managed Windows Distribution

- The managed install uses exactly one stable launcher at `bin/herdr.exe` and
  immutable `runtime/<build-id>` payloads. The launcher starts the selected payload
  directly; runtime directories contain no dispatcher or second launcher. Strict
  Active/Pending records and per-build leases prevent old and new runtime components
  from mixing while sessions remain active.
- The launcher owns runtime selection, process forwarding, lease inheritance, and
  opportunistic activation after the final old lease. On a normal payload exit, it
  invokes the installed native helper at `state/installer-helper.exe` only when
  pending launcher publication or runtime pruning is needed. It does not terminate
  user sessions or require a service, reparse point, reboot replacement, or
  background poller.
- Setup serializes launcher changes with `state/launcher.lock`. It replaces an idle
  launcher atomically or writes one hash-addressed `launcher.pending-<sha256>.exe`
  plus a strict pending record while the launcher is in use. The post-exit helper
  waits for its launcher parent, validates the pending hash and private build ID,
  then publishes through the existing staging/backup boundary without first
  deleting the working launcher. Setup and later safe exits repair a hard-kill
  interruption from the same pending state.
- Active plus optional Pending are the only retained runtime identities. Under the
  existing coordination lock, post-exit/setup maintenance removes
  every other exact runtime whose lease can be acquired exclusively. Busy,
  malformed, reparse-point, or otherwise ambiguous content is preserved and causes
  the maintenance attempt to report failure rather than broadening deletion.
- The Windows package has three Rust executables: the user-facing runtime, the stable
  launcher, and the internal installer helper. NSIS owns the setup/uninstall shell,
  embedded inputs, and user-visible progress/error boundary. It runs the embedded
  temporary native helper and never mutates the installed root directly. The helper
  owns final root cleanup while holding the lifecycle mutex, filesystem
  validation, launcher publication, runtime pruning, PATH/Installed Apps integration,
  optional user-settings removal, and recoverable install/uninstall state. Rust also
  owns runtime selection and downloading/verifying/launching the immutable installer
  asset.
- NSIS forces its embedded CRC check and recognizes package-manager and destructive
  custom flags only as exact tokens.
- The helper validates managed Installed Apps registry kinds and content before
  mutation or key removal; unknown values or subkeys are preserved and rejected.
  Setup accepts the complete current value set plus the exact preceding current
  sets that omit `PathValueCreated` or both PATH ownership values. Missing values
  mean unowned, and successful setup rewrites the complete current set. Uninstall
  and registration removal accept only those validated identities and the current
  native-helper quiet-uninstall command. There is no prior command, script helper,
  or bridge acceptance path.
- Installed Apps quiet uninstall invokes `state/installer-helper.exe` directly.
  That process creates one random strict sibling rendezvous, starts the NSIS
  uninstaller in a bounded native job, and waits for the embedded helper's terminal
  result. The embedded helper accepts only that exact installed-helper process,
  moves its locked image to the validated sibling handoff path, removes payload and
  installer-owned registration, then finalizes the managed root. A nonterminal
  cleanup fault restores the actual helper and uninstaller bytes, final ownership
  marker, exact installer-owned PATH bytes, and exact Installed Apps values before
  reporting failure. The helper publishes the result and deletes its handoff after
  the waiting process exits. Fresh installs, updates, quiet uninstall, and post-exit
  maintenance therefore ship and invoke no PowerShell payload.
- For a normal exact managed root, uninstall validates the root while holding the
  lifecycle mutex, resolves Active, opens that build's shared lease, and invokes
  `runtime/<active-build-id>/herdr.exe` directly with one exact hidden shutdown
  token. It never enters the stable launcher, so shutdown cannot activate Pending
  state before launcher coordination. The runtime enumerates default and named
  sessions and requests graceful server stops under one 30-second total deadline.
  The helper contains that command in a private job with a 35-second deadline and
  terminates only the command job on timeout. Any session that remains reachable,
  or any active or ambiguous installed process or lease, aborts uninstall before
  destructive mutation.
- NSIS accepts `/WINGET` as the sole explicit package-manager origin signal and
  passes a bounded Direct/WinGet value into the helper. The helper owns the optional
  strict UTF-8 `state/package-manager` record; only the exact
  `herdr-package-manager-v1\nmanager=winget\n` bytes establish WinGet ownership.
  Fresh WinGet setup publishes it with the managed tree, repeated WinGet or direct
  setup preserves it, and uninstall removes it. Rust derives ownership only through
  the validated current managed-install root; malformed or unreadable records fail
  closed rather than being inferred from PATH, ARP, process ancestry, or WinGet
  metadata.
- The existing startup update thread remains feed-driven for direct installs. For
  an exact WinGet-owned root, it takes the target CalVer from the validated preview
  feed and runs one bounded, non-interactive `winget show` query for that exact
  package, official source, version, x64 architecture, and user scope. The query is
  contained in a kill-on-close Windows job. Only success publishes release notes and
  an update event; package/version absence is pending, while launch, source, timeout,
  containment, and other failures suppress the action and are logged. No polling,
  second availability feed, or package-manager state is added.
- One stable unversioned Windows named mutex,
  `Local\HerdrWinInstallerLifecycle`, serializes every installer generation and is
  the permanent interoperability name across releases.
  The helper calls `CreateMutexW` without initial ownership and attempts ownership
  once with `WaitForSingleObject(0)`. A live owner blocks the operation, an abandoned
  owner is recovered, and the owning thread releases the mutex before closing its
  handle. Object existence, PID-qualified names, polling, and stale-file deletion
  never establish lifecycle ownership.
  Fresh and update work uses uniquely named same-parent staging only; once the lock
  is acquired, any stale regular staging tree is disposable and its grammar never
  decides whether the user's requested operation may continue. Cleanup failure
  preserves that private staging with a warning. Uninstall has no sibling
  transaction, cleanup manifest, or rollback parser. Before taking the launcher
  gate, a normal exact root completes the bounded session shutdown above. Under the
  gate it validates active processes and leases, publishes
  `state/uninstall.pending`, then atomically renames each validated `bin` and
  immutable runtime directory into disposable same-parent uninstall staging before
  best-effort deletion. Interruption therefore cannot leave a partially deleted
  managed directory. The remaining exact residual keeps that marker as its final
  filesystem ownership sentinel while PATH and Installed Apps cleanup runs with
  both retry commands still present. Final cleanup enumerates the residual again
  before removing the marker. Bounded in-memory snapshots restore only the actual
  control bytes and exact registry state changed by this attempt; they add no
  persistent journal or second recovery authority.
- Payload launch independently acquires `state/launcher.lock`, validates the stable
  launcher, and rejects any present `state/uninstall.pending` marker before runtime
  selection or `CreateProcess`. Thus a killed uninstaller cannot admit a new session
  between releasing its process-owned lock and a later recovery pass; the existing
  launcher is reused and no service or second launch path is added.
- The packager owns installer-facing product identity inputs and validates the
  runtime, launcher, and native helper as three distinct x64 executables. It passes
  one runtime product name, one title-cased human distribution display name, the
  release CalVer or explicit local identity, the upstream Herdr version, and the
  fork and official-upstream URLs into NSIS. The
  runtime/install-root identity remains Herdr, while executable
  metadata and Installed Apps consistently present **Herdr Win**. The NSIS presentation
  uses standard MUI2 Welcome/License/Files/Finish pages plus the existing custom
  uninstall choice. Window, Welcome, progress, and Finish presentation reuse that
  one display name; Welcome and Finish titles reuse the same short herdr-win
  release identity or explicit local marker.
  Welcome identifies the unofficial stable-plus-patches distribution without
  displaying its fixed path. Finish exposes separate user-invoked fork and upstream
  links and never launches Herdr or a browser automatically. Root `LICENSE` is
  projected once as payload `LICENSE.txt`; that exact file owns both the License
  page and the copy installed beside the product. One high-resolution source owns
  the branded Welcome/Finish artwork; five checked-in BMP3 derivatives provide
  native 100–200% DPI buckets without runtime resampling. Installer compression
  uses datablock optimization, an 8 MiB LZMA dictionary, and solid final LZMA settings.
  Packaging runs the pinned NSIS compiler with warnings as errors and without
  ambient NSIS configuration, writing its candidate in a private regular sibling
  of the requested output. Only a validated x86 PE is atomically moved or replaced
  into the final path, so failure leaves an existing regular artifact intact.
  Success reports the absolute path, byte count, SHA-256, and compiler version.
  There is no replaced-executable definition or compatibility slot; the fixed
  launcher and helper package leaves remain owned directly by the one package
  schema.
- Install/update preserves the raw user-PATH registry kind and bytes. It adds the
  literal managed `bin` path first only when no effective equivalent exists, records
  that exact ownership in ARP, and never claims or rewrites an equivalent user-owned
  spelling. Immediately before the registry write it publishes the strict
  `state/path-add.pending` ownership intent, including whether the `PATH` value was
  absent and had to be created. Successful ARP publication removes the
  marker; if publication stops after `PathAdded` but before `PathValueCreated`, the
  exact pending marker completes only that missing fact on retry. Setup or uninstall
  recovery may claim only an exact literal entry while that marker remains.
  Uninstall compares each raw unexpanded candidate and removes every rooted
  literal spelling that normalizes to the fixed managed path, including case,
  slash, quote, and trailing-separator variants. A literal owned path may contain
  `%`; an expandable token such as `%LOCALAPPDATA%\...` remains foreign even when
  it resolves to the managed directory. Every other entry and its order remain
  unchanged, and the value is deleted only when setup created it and no concurrent
  entries remain.
- Interactive and silent uninstall both preserve `%USERPROFILE%\.herdr` by
  default; the interactive checkbox or `/REMOVE_SETTINGS` explicitly authorizes
  deletion. Settings cleanup stays in the helper's validated filesystem boundary
  and never follows ambiguous/reparse-point content. It runs after managed skill,
  application, PATH, and Installed Apps cleanup; an unsafe or locked settings
  residual is preserved and reported without changing successful application
  removal into failure. The managed install/runtime tree is not best-effort state:
  its active process and lease checks remain required uninstall gates.
- `src/distribution.rs` is the single fork update-source configuration. New
  Windows clients consume the separately hashed immutable NSIS asset from the fork
  release; there is no upstream-source fallback. The portable ZIP remains only as
  a user choice and compatibility asset for older immutable clients.
- The managed setup accepts only a missing install root or the exact current
  managed layout. Any other root is preserved and rejected with an uninstall-first
  action; there are no migration, compatibility, or backup branches. In particular,
  a runtime-local `herdr-launcher.exe` marks the former two-hop layout and is rejected
  before repair, PATH, or ARP mutation. The user removes its existing **Herdr** entry
  before a fresh **Herdr Win** install, so setup never co-owns duplicate package
  registrations.
- Exact ARP ownership plus the current bin sentinel and install manifest permit
  repair of the installer control filenames only: missing helper or uninstaller
  files are recreated, and changed regular files are atomically
  replaced through the native `ReplaceFileW` boundary with backups of their actual
  current bytes. When this
  normal path cannot classify a root already bound by exact current registration,
  the same helper treats the dedicated managed root as a complete convergence root:
  it holds available launcher coordination, rejects active processes, leases, and
  reparse points, removes the old root, and either publishes the requested current
  build or completes removal. This is current-format convergence, not legacy
  adoption or a published-hash bypass.
- `packaging/windows/managed-skill-hashes.txt` is the one append-only ownership
  manifest for the current and every historically installer-delivered
  `skills/herdr/SKILL.md` byte hash. The packager validates that the current payload
  is present; NSIS embeds the payload and the same manifest into setup and uninstall
  without adding either to the persistent managed-root layout. The native helper
  copies a missing skill, replaces a known hash, and preserves an unknown regular
  file while returning a visible setup warning.
- Skill inspection is pure. Across the universal root and configured/default Claude
  roots, only all-known-or-absent state selects the interactive removal checkbox;
  unknown or ambiguous state leaves it clear. Interactive selection or
  `/REMOVE_SKILL` authorizes exact unknown `SKILL.md` removal, while silent automatic
  cleanup removes only known hashes. A skill directory is removed only after an
  authorized file removal and an empty-directory check. Foreign siblings are never
  recursively deleted, and reparse points or ambiguous collisions remain preserved
  without per-install skill markers, transactions, or locks.
- Future installer work uses the global ordinary-local-application threat model
  unless the user explicitly chooses stronger guarantees. Measure success by the
  requested installed/current or absent terminal state. Private staging and
  malformed recovery metadata are never product-level blockers; preserve only true
  unsafe boundaries such as reparse escapes, active installed processes, and live
  runtime leases.

## Windows and Rust Boundaries

- Windows PTY integration owns process-tree cleanup, handle inheritance, resize,
  and byte-stream behavior explicitly; Windows-only code compiles only on Windows.
- Native/FFI code remains narrow, reviewed, and justified by an exact operating
  system boundary. Controlled internal Rust paths stay direct and propagate
  contextual errors without production `unwrap`/`expect`.
- Async work remains cancellation-safe and does not hold non-async locks across
  `.await`. Production diagnostics use `tracing`.

## Release and Generated State

- `.github/workflows/ci.yml` owns cheap replay validation.
  `.github/workflows/release.yml` owns one manually dispatched workflow with two
  explicit operations: build a retained cross-platform candidate, or promote one
  successful candidate run without rebuilding it. No event or schedule invokes
  either operation automatically.
- A build dispatch requires a herdr-win CalVer `YYYY.MM.DD.N`. It runs Windows
  native/package tests and the four upstream-supported Linux/macOS executable
  builds. Every platform job independently replays recorded `BASE` and the queue;
  each source tree must match the tree tested by the Windows owner job, while one
  shared candidate-scoped build ID and protocol value identify all assets. The
  build ID combines the upstream prefix with a hash of the selected control commit,
  workflow run ID, and attempt, preventing byte-distinct rebuilds of unchanged
  source from colliding in one immutable runtime directory.
- A successful build retains its candidate artifacts for 14 days. The Windows
  candidate owns `RELEASE_CANDIDATE.json`, which records the workflow run and
  attempt, CalVer, source/control identities, protocol, and exact expected release
  filenames. Each candidate asset has a SHA-256 sidecar. Candidate creation does
  not create a tag, release, manifest commit, or other published channel state.
- A promotion dispatch requires only the candidate build run ID. It fails closed
  unless that run completed successfully in this repository and workflow on the
  still-current `master` control commit, its metadata identifies that run and a
  valid attempt, all source identities and filenames are coherent, and the complete
  candidate file set matches every sidecar digest. Promotion downloads those
  retained artifacts and publishes the exact bytes; it contains no source replay,
  compile, or package path. Only the promotion job receives Actions read and
  repository write permissions.
- The promoted release tag is `v<CalVer>`. Linux and macOS publish raw executables named
  `herdr-win_v<CalVer>_{linux,macos}_{amd64,arm64}` for direct remote installation.
  Windows keeps `herdr-win_v<CalVer>_windows_amd64.zip` and the corresponding
  `_setup.exe`. The three packaged Rust executables statically link the MSVC runtime,
  so the per-user setup and portable archive require no separately installed Visual
  C++ redistributable. The manifest's upstream-compatible target keys remain separate
  from these fork-presented filenames.
- Promotion publishes those six target artifacts without candidate checksum
  sidecars. Candidate sidecars remain internal verification inputs; GitHub records
  SHA-256 for every immutable release asset and the update manifest carries the same
  six verified digests.
- A WinGet community submission is generated only after immutable promotion and
  remains external release output under ignored `target/`, not a second checked-in
  package tree. Its multi-file manifest uses package ID `hdosys.herdr-win`, the
  CalVer as `PackageVersion`, x64 per-user NSIS metadata, the exact immutable setup
  URL and GitHub-recorded SHA-256, and silent switches `/S /WINGET`. Neither the
  release workflow nor ordinary repository delivery submits the external PR.
- Publication and the generated manifest proceed only when all six target assets
  have verified SHA-256 digests; retained historical manifest entries may remain
  Windows-only.
- Machine-consumed tags and asset filenames remain CalVer-only for existing updater
  compatibility. The GitHub release title is `herdr-win v<CalVer> (Herdr
  v<upstream-version>)`; release notes and installer metadata expose that same
  stable upstream version alongside the CalVer-bearing original filename.
- Every published binary receives that same CalVer as compile-time release identity.
  `herdr --version` renders `herdr-win <CalVer> (Herdr <upstream-version>)`, and
  Windows setup, executable resources, and Installed Apps use the CalVer as their
  primary display version. Local builds carry no CalVer and render `herdr-win local
  (Herdr <upstream-version>, build <build-id>)` when build provenance is available.
- The runtime build ID remains two lowercase 12-hex components because it owns
  managed-runtime identity: the upstream prefix plus the candidate identity hash.
  Full upstream/control hashes and run/attempt metadata remain the exact provenance
  owners. CalVer owns the human fork release identity; the upstream Cargo version
  remains compatibility/provenance metadata and does not define the herdr-win release.
- The compiled CalVer also owns update ordering and release-note state. Runtime
  status and handoff use `<CalVer>+<build-id>` for published candidates and
  `local[+<build-id>]` for local builds so exact payload matching remains separate
  from release order. Client status reports upstream Herdr SemVer and build ID as
  separate fields. The fork exposes no user-selectable update channel or
  `update.channel` config path.
- `website/preview.json` is generated channel state and the promotion operation is
  its only writer. Release publication uses the exact tested candidate and fails
  closed on source drift, stale control state, missing/mutable assets, or
  digest/feed mismatch.
- Root `README.md` and `docs/next/README.md` are byte-identical public projections;
  they do not replace `PRODUCT.md` or this architecture owner.

## Verification Architecture

- Control-plane inventory tests validate BASE, series order, mailbox identity, and
  frozen archive invariants before product gates.
- Verification has distinct control-plane inventory, replayed-product, and
  native/package lanes. `CONTRIBUTING.md` owns when each lane runs and keeps broad
  gates on one frozen logical snapshot.
- Formatting, Clippy, and Rust tests run in replayed product source. Cross-platform
  release builds add native target/machine checks and static-link validation for
  Linux. Windows packaging changes add package/vendor checks, native quiet-uninstall,
  helper and launcher tests, and realistic installer evidence where that boundary
  changed.
- Broad gates run on an implementation-frozen snapshot. Passing evidence remains
  valid until relevant source, inputs, or environment-sensitive assumptions change.
