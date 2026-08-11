# Settled Decisions (do not re-flag without reading this)

These have come up as apparent inefficiencies during codebase audits and were deliberately
kept as-is. Before proposing a change here, read the full rationale below — CLAUDE.md links
to this file specifically so these aren't re-litigated from scratch.

## Intentional Designs (do not "optimize" these)

These have come up as apparent inefficiencies during codebase audits and were deliberately kept
as-is. Don't re-flag or "fix" them without understanding why first:

- **Backend credentials live in their own nullable `credentials_nonce`/`credentials_ciphertext`
  columns, not folded into the existing `password_ciphertext` blob as a single JSON envelope.**
  A single-envelope design (`{"password": "…", "credentials": {...}}` all under one
  encrypt/decrypt) would make `rotate_master_key` correct with zero rotation-specific code for
  credentials, which reads as an obvious simplification — resist it. It requires migrating every
  existing repo's password ciphertext into the new format, and `rotate_master_key`'s own doc
  comment already warns that a partial failure there "would lock the user out and brick every
  repo": the envelope trades a recoverable failure mode (a credential re-encrypt bug breaks
  credentials, which can be re-entered) for an unrecoverable one (a migration bug breaks the
  password itself, for every repo). The separate-columns design gets the same "rotation can't
  silently orphan a secret" guarantee a different way — a post-rotation verification pass that
  re-reads every row through `decode_secrets` with the new key before committing (see Security
  Architecture) — without ever touching ciphertext that already works.
- **`repo::apply_backend_env` filters reserved (`PATH`/`RESTIC_*`) credential keys itself,
  rather than relying on `backends::validate_credentials` having already rejected them.**
  This reads as redundant — every credential that reaches `apply_backend_env` is supposed to
  have passed `validate_credentials` first (`add_repo`, `init_repo`, `update_repo_secrets`,
  `test_repo_connection`, and `import_data`) — and it would be tempting to drop the filter and
  trust that invariant. Don't: `validate_credentials` only covers rows that went through one of
  those entry points; nothing stops a row reaching the `repositories` table another way (a
  future ingest path that forgets to validate, or a hand-edited database), and every real call
  site sets `RESTIC_REPOSITORY` (`RESTIC_FROM_REPOSITORY` for copy/mirror's source side) on the
  `Command` *before* calling `apply_backend_env` — so an unfiltered reserved-key credential
  would win that collision and silently redirect the operation to a different repository. The
  filter inside `apply_backend_env` is what makes the guarantee hold unconditionally instead of
  depending on every ingest path remembering to validate first; `validate_credentials` remains
  valuable as the earlier, loud rejection a user actually sees. The filter now carries one
  exception, `backends::ALLOWED_RESTIC_KEYS` (`RESTIC_REST_USERNAME`/`RESTIC_REST_PASSWORD`) —
  see the "Backend credentials" bullet's REST paragraph above — safe specifically because the app
  never sets either itself, so there is no `RESTIC_REPOSITORY`-style collision for a stored value
  to win.
- **`backends::validate_credentials` deliberately excludes `BackendKind::Rest` from the
  S3/B2-only unknown-key check**, even though `Rest` now has its own `REST_SPECS` (the two
  `RESTIC_REST_*` vars) the same way S3/B2 have theirs. Adding `Rest` to that check looks like
  the obvious next step once a dedicated spec table exists — resist it: every `rest:` repo added
  before `BackendKind::Rest` existed was classified `BackendKind::Other`, which has no unknown-key
  check at all, so such a repo may already store an arbitrary credential (e.g. `HTTPS_PROXY`).
  Tightening `Rest` to reject unrecognized keys would fail that repo's validation on its very next
  edit, connection test, or import — a regression with no user action to blame it on. Pinned by
  `validate_credentials_allows_arbitrary_keys_for_rest` (`backends.rs`).
- **`"rest:"` stays listed in both `REMOTE_PREFIXES` arrays (`backends.rs` and `backends.ts`)
  even though `detect_kind`/`detectBackend` now match it in a dedicated arm ahead of that list.**
  It reads as dead weight once the arm exists — don't remove it. Both arrays are also the
  documented manual mirror of `isRemoteRepo` (`src/lib/types.ts`), which gates
  `remote_auto_refresh`: the cache warmer's 60s snapshot sweep, its auto-indexing sweep, and
  SnapshotsPage's background refresh all skip a repo `isRemoteRepo` says is remote unless that
  setting is on. Dropping `"rest:"` from either `REMOTE_PREFIXES` copy wouldn't change
  `detect_kind`'s own output (the dedicated arm already runs first) but would silently make every
  REST repo start being treated as local everywhere else that list is consulted. Pinned by
  `detect_kind_rest_still_counts_as_a_remote_prefix` (`backends.rs`).
- **Sync `#[tauri::command]`s are intentionally not wrapped in `spawn_blocking`.** Tauri runs
  non-`async fn` commands (e.g. `get_restic_version`, `list_repos`) on its own thread pool, off
  the async runtime entirely — only `async fn` commands that block need `spawn_blocking`.
- **`scheduler.rs` and `schedule.rs`'s `run_schedule_now` call the *sync* `apply_retention`
  directly, not through `spawn_blocking`.** Both run inside their own background
  `tauri::async_runtime::spawn`ed tasks (not foreground commands), immediately after
  `execute_backup` (which already does its heavy work via `spawn_blocking`). Only the foreground
  `forget_by_plan` command wraps `apply_retention` in `spawn_blocking`.
- **`list_snapshots`, `get_snapshot_index_status`, and `get_repo_stats` don't emit on the `task`
  event bus (see Operation Event Bus)** because none of the three shells out to restic — nothing
  runs that a task could represent. **`cache_warmer`'s `refresh_all_snapshots` tick also doesn't**,
  despite calling restic, because it fires automatically every 60s forever rather than being
  bounded by a user action — wiring it would mean unbounded event volume over a long session.
  Every other restic-shelling command, including click-bounded metadata reads like
  `refresh_repo_stats`/`get_snapshot_stats`/`test_repo_connection`/`list_files`, is wired. Don't add
  the excluded four without revisiting that tradeoff.
- **`get_repo_stats` fetches from `repo_stats_cache` for *all* repos including remote ones, and
  RepositoriesPage requests it for every repo on mount — on purpose — but it is a pure cache read
  and must never fall through to a live `restic stats` call on a miss.** It used to (returning
  freshly-fetched stats on a cache miss, matching `refresh_repo_stats`'s behavior), which seemed
  harmless — a miss should only ever happen for a repo that had genuinely never been fetched — until
  "Clear All Cache" (`AppDb::clear_cache`, SettingsPage) started wiping `repo_stats_cache` for every
  repo at once: the very next RepositoriesPage mount then silently kicked off a real `restic stats`
  subprocess for every single repo, auto-refreshing a feature that's supposed to be manual-only the
  moment its cache was cleared (a confirmed regression, not hypothetical — see repo.rs's doc comment
  on `get_repo_stats`). It now returns `Err` on a miss instead (the frontend's existing "couldn't
  load" `—` placeholder covers this, same as any other failed fetch) — populating a repo's stats,
  including right after a first add or a cache clear, is exclusively `refresh_repo_stats`'s job now.
  The `—` placeholder in the UI is for exactly this: a remote (or any repo) that has no cache yet.
  Do not skip remote repos in the mount-time fetch, and do not reintroduce a fetch-on-miss fallback
  here — it would hide cached remote stats that are otherwise perfectly valid to show, and it would
  reopen the auto-refresh-after-Clear-Cache regression respectively. RepositoriesPage's manual
  "Refresh All"/per-row Refresh buttons (`refresh_repo_stats`) likewise always include remote
  repos — unlike every *automatic* remote activity (cache warmer's snapshot/index sweep,
  SnapshotsPage's background refresh, Index All), which stay gated behind `remote_auto_refresh`, a
  manual refresh is an explicit user request with no surprise-bandwidth concern to guard against.
- **`browse_cache_files.parent_path` duplicates a prefix of `path` on every row, on purpose.** It
  backs the `(snap, parent_path)` directory-listing index — a deliberate storage-for-speed
  trade-off, and the single largest contributor to that table's size. Acceptable.
- **File search (`search_browse_files`/`search_repo_files`) uses `path LIKE '%query%'`** — the
  leading wildcard means SQLite can't use the index and does a full scan. This is a known,
  accepted cost (not an oversight): it's exactly why those two search commands are `async` +
  `tauri::async_runtime::spawn_blocking` + guarded by `searchSeqRef` on the frontend. An FTS5 or
  trigram index would fix the underlying scan but needs a schema migration — a deliberately
  deferred future improvement.
- **`cached_at` columns (`snapshots_cache`, `browse_cache_status`) are written on every update but
  not currently read by any query.** They're kept for a possible future staleness/TTL feature;
  today, staleness is handled entirely by explicit refresh/evict calls. Not dead weight to be
  dropped without that feature landing. `repo_stats_cache.cached_at` is the exception — it now has
  a reader: `get_stats`/`set_stats` (`cache.rs`) return it, and `ResticStats.cached_at` surfaces it
  as RepositoriesPage's "Refreshed …" label (see Restic Integration).
- **`panic = "abort"` is deliberately not set** in `src-tauri/Cargo.toml`'s release profile (see
  Build Profile). The code is written to survive worker-thread panics — `spawn_blocking` results
  are handled via `.unwrap_or(false)` patterns, and `AppDb`'s `Mutex<Connection>` poison errors are
  mapped to recoverable `Err`s rather than propagated as panics. `panic = "abort"` would turn a
  survivable background-thread panic into a full-app crash.
- **Known, deferred (not novel) frontend duplication:** the search/index/debounce pattern, the
  `FileIcon` component, and the `browseTarget` helper are each duplicated across
  `SearchPage.tsx`, `RepoSearchPage.tsx`, and (partially) `BrowsePage.tsx`/`DiffPage.tsx`;
  `RepoSearchPage` re-subscribes its `task` (index) listener on every keystroke; every page
  independently calls `listRepos()` on mount instead of sharing a cache; `BrowsePage` renders a
  directory's full entry list with no pagination or virtualization; the "Index All Snapshots"
  batch-tracking state machine and progress modal (queued/running/stopped/complete, `task`
  listener, `getActiveIndexBatch` adoption) is duplicated between `RepoSearchPage.tsx` and
  `RepositoriesPage.tsx`'s context-menu equivalent. All are known and intentionally
  deferred (structural refactor / new dependency required) — revisit deliberately, don't
  rediscover them as "new" findings.
- **Backup progress bars are non-monotonic by design.** restic's `percent_done` (= `bytes_done /
  total_bytes`) fluctuates early in a run — restic scans the directory tree concurrently with
  uploading, so `total_bytes` grows as more files are discovered, which inflates the ratio (the bar
  shoots up) and then drops it as the denominator grows, before finally climbing to 100%. Both the
  Activity panel (`ActivityPanel.tsx`) and the manual backup modal (`BackupPlansPage.tsx`) display
  `percent_done` raw — `execute_backup` parses it straight from restic's `status` lines
  (`snapshot.rs`). Investigated fixes — a monotonic high-water mark, or an indeterminate bar during
  the scan phase then determinate — were deliberately **not** applied: a high-water mark would latch
  onto the early spike and stall near it for the rest of the run, looking "stuck", which is worse
  UX than the self-correcting fluctuation (it always lands at 100%); the indeterminate variant needs
  a new `ProgressBar` + a scan-stabilization heuristic. Don't re-investigate without revisiting that
  trade-off.
- **`IndexHandle::gate` must stay a single, app-wide `tokio::sync::Mutex<()>` — never split it
  per-batch, per-repo, or otherwise widen indexing concurrency.** Pre-v0.2.1, "Index All" fanned
  out one concurrent `index_snapshot` call per snapshot (each spawning its own `restic ls`
  process, each materializing a full file list in memory) — reported to use 33GB RAM and crash
  the app on large repos (see commit `31b7240`). `gate`, held across every `run_full_index` call
  from every caller (`index_snapshot`, `index_snapshots_batch`'s per-snapshot loop, and
  `cache_warmer`'s auto-indexer), is what guarantees strictly one indexing process runs app-wide,
  ever — this is the actual fix, not batching or sequencing by itself. `IndexHandle::batches`
  (the per-batch cancel-flag/task-slot registry that lets multiple "Index All" runs, e.g. for
  different repos, be tracked and cancelled independently in the Activity panel) is a *bookkeeping*
  change layered on top and does not affect this: every batch still calls `gate.lock().await`
  before each `run_full_index`, so N concurrently-running batches still only ever have one
  `restic ls` in flight at a time, taking turns snapshot-by-snapshot through the same mutex — the
  memory ceiling is unchanged from post-v0.2.1. Do not "simplify" by giving `gate` per-batch scope
  to let batches truly run in parallel; that reopens the exact incident this mutex exists to
  prevent.
- **`gpu_compat::apply()`'s NVIDIA + Wayland detection is gated on purpose, not left
  unconditional.** See Linux GPU Compatibility. The straightforward fix would be to always set
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` on Linux — simpler code, no detection logic to maintain —
  but Tauri's own docs warn that doing so "disables a faster path for everyone, including users
  on working setups," and this app has no reason to slow down Intel/AMD or X11 users to fix an
  NVIDIA/Wayland-specific bug. Don't collapse the gate to "just always set it on Linux" without
  revisiting that trade-off. Also don't add `WEBKIT_DISABLE_COMPOSITING_MODE` alongside the
  other two "for completeness" — it's the most expensive of Tauri's three options and targets a
  symptom (crash-on-resize) nothing here has actually reported.
- **Launch-at-login (`repo::get/set_launch_at_login`) has no `app_settings` row, unlike every
  other Settings toggle in this file.** `tauri-plugin-autostart`'s `is_enabled()` reads the real
  OS entry (macOS LaunchAgent plist / Windows HKCU Run value / Linux XDG `.desktop`) directly, so
  the toggle can never drift from what the OS will actually do on next login — including when the
  user deletes the entry outside the app. Mirroring the `tray_enabled`/`auto_indexing` pattern
  with a DB row here would reintroduce exactly the drift the plugin's own storage already avoids,
  and would need reconciliation logic on every read. Don't add one "for consistency" with the
  other toggles. **Auto-unlock (`auth::get/set_auto_unlock`) deliberately does the opposite** and
  keeps its own `app_settings` row (`auto_unlock`) as the toggle's display state, even though it
  is exactly the kind of "reads the real backing store" toggle this bullet argues against
  mirroring with a DB row. The difference is that reading the real backing store here means a
  keychain read, which — unlike an autostart file/registry check — is neither cheap nor silent:
  on macOS it can raise a permission dialog. Deriving display state from it would prompt the user
  every time Settings mounts. The row is UI state only; the keychain remains the sole store of
  the secret itself, and `try_auto_unlock`/`set_auto_unlock` are written so the row never claims a
  state the keychain doesn't actually back (see Security Architecture).
- **The auto-unlock toggle is deliberately *not* gated on launch-at-login or the tray setting,
  unlike launch-at-login's own gating on the tray.** The tray → launch-at-login gate exists for a
  hard functional reason: without the tray, closing the window quits the app, so launch-at-login
  genuinely doesn't work without it. Auto-unlock has no such dependency — it benefits a user who
  opens Resty manually exactly as much as one whose OS opens it. Gating it would also create a
  destructive cascade: `SettingsPage.tsx`'s `handleTrayToggle` force-clears launch-at-login on
  *both* tray transitions (see the bullet below), and anything hanging off launch-at-login
  inherits that — so toggling the tray would reach two levels down and delete the user's keychain
  entry as a side effect of an unrelated setting. Leaving auto-unlock ungated means nothing to
  inherit, and `handleTrayToggle` needed no changes to support this feature.
- **`.app_name("resty-desktop")` on the `tauri_plugin_autostart::Builder` must not be dropped.**
  It defaults to `package_info().name`, which for this app is `"Resty Desktop"` — with a space —
  and the pinned `auto-launch 0.5.0` writes both the Linux `Exec=` line and the Windows Run
  registry value **unquoted**. Losing the explicit name reintroduces a broken/ambiguous autostart
  entry on those two platforms.
- **A login launch shows the window on the unlock screen; it does not launch hidden into the
  tray.** The app always starts locked (`MasterKey` is in-memory only, no auto-unlock command
  exists), and `scheduler.rs`'s tick silently no-ops while locked — so a hidden launch would park
  a locked app in the tray running nothing until the user found and clicked the tray icon,
  advertising background activity ("launch at login") the app cannot actually perform yet. A
  visible unlock screen gets the user to unlock (and backups to actually resume) immediately, at
  the cost of not being a silent background start. Revisiting this requires first designing an
  auto-unlock story, which is a security decision, not a startup one. **That story is now the
  opt-in, default-off auto-unlock toggle** (see Security Architecture) — a login launch still
  always shows the window (there is no silent/hidden launch path even with auto-unlock on), but
  `try_auto_unlock` now runs before the unlock screen renders, so a user who has opted in sees
  the repository list directly rather than the password prompt, and scheduled backups resume
  without anyone finding and clicking anything. A user who hasn't opted in sees no change at all.
- **`SettingsPage.tsx`'s `handleTrayToggle` clears launch-at-login on *enabling* the tray, not
  only on disabling it.** Clearing only on disable looks sufficient at first glance — it does stop
  an orphaned OS entry from surviving after the tray (and thus hide-to-tray) goes away — but it
  would let the launch-at-login toggle silently inherit whatever OS-level entry happened to
  already exist the moment the tray is turned back on. Clearing on both transitions guarantees the
  now-interactive toggle always starts from a known "off" state. That clear call is deliberately
  placed *after*, not inside, the tray operation's own `try`/`catch` — by the time it runs, the
  tray setting is already persisted and the tray icon already created/removed, so a failure here
  must not roll `trayEnabled` back (that would make the UI contradict both the DB and the real
  tray); it reports its own error and re-reads `is_enabled()` instead of assuming the clear
  landed, since this setting has no `app_settings` row backing it to fall back on.
- **`repo::set_launch_at_login`'s disable path checks `is_enabled()` before calling
  `disable()` — this is not redundant defensiveness, do not remove it.** `auto-launch 0.5.0`
  guards its own disable path with `file.exists()` on macOS and Linux (`if file.exists() {
  fs::remove_file(file)?; }`) but *not* on Windows, where it calls `RegDeleteValueW`
  unconditionally via `winreg`'s `delete_value`, which errors `ERROR_FILE_NOT_FOUND` when the
  `Run` value isn't there. `handleTrayToggle` (`SettingsPage.tsx`) calls this setter with
  `value: false` on *every* tray toggle, including for the overwhelming majority of users who
  have never touched launch-at-login — so without this guard, toggling the tray at all would
  fail on Windows for every such user. The `is_enabled()` check makes the setter idempotent on
  all three platforms instead of relying on Windows' underlying API to already be. A failed
  `is_enabled()` read is treated as "skip the disable" rather than propagated — a disable this
  function can't even confirm is needed isn't worth failing the caller over.
- **`auth::reset_app` also best-effort clears the autostart entry, not just `AppDb::reset_all`'s
  tables.** `reset_all` wipes `app_settings` (so `tray_enabled` reverts to its `false` default),
  but the autostart entry lives entirely outside the DB — without this, a reset would leave the
  OS launching the app at every login into the first-launch setup screen, with the Settings
  toggle now rendering off (`trayEnabled && launchAtLogin` is false once the tray reverts) and no
  way to clear it short of re-enabling the tray first. The call's result is discarded (`let _ =`)
  deliberately: wiping user data is the part of a reset that must not fail, and
  `set_launch_at_login` is already idempotent for exactly this kind of best-effort call.
- **The Windows `Run` registry value `auto-launch` writes is unquoted, and this cannot be fixed
  from app code.** `auto-launch 0.5.0`'s Windows `enable()` does
  `format!("{} {}", app_path, args.join(" "))` with no quoting. With this app's empty `args` that
  is just a trailing space after the path, which `CreateProcess`'s space-resolution heuristic
  does correctly resolve in practice — but it is a known-fragile shape (a binary earlier in the
  path, e.g. `C:\Program Files\Resty.exe`, would be tried first). Writing to
  `HKCU\...\CurrentVersion\Run` needs no elevation, but planting such a binary does, which is why
  this is accepted rather than treated as a blocker. Not fixable without forking `auto-launch` or
  dropping the plugin; `.app_name("resty-desktop")` already removes the space from the *entry
  name*, which is the one part of this we control.
- **`react-router-dom` stays on the 6.x line — its two current `npm audit` advisories (an open
  redirect via a backslash in `<Link>`/`useNavigate`, and arbitrary constructor injection via
  `deserializeErrors()` during SSR hydration) are unreachable here and don't justify the v7
  migration.** Both need conditions this app never creates: the SSR one needs server-side
  rendering, which a local Tauri app doesn't do at all; the open-redirect one needs an
  attacker-controlled navigation target, and no `navigate()`/`<Link to>` call in this codebase is
  ever built from one — every interpolated path segment (e.g. `` `/snapshots/${repoId}` ``) comes
  from an internal id already resolved from this app's own data (a repo, plan, or snapshot id),
  never from a URL query param, `location.search`, or other external input. Re-flagging this from
  a future `npm audit` run without checking reachability first would push a breaking
  major-version migration (v7 changes routing APIs across all 13 routed pages) for zero actual
  risk reduction.


## Linux GPU Compatibility

## Linux GPU Compatibility

`src-tauri/src/gpu_compat.rs` works around a known WebKitGTK/NVIDIA/Wayland crash (`Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.`, reproduced by users on Fedora + NVIDIA + Wayland) by setting the same env vars Tauri's own [Linux Graphics Issues](https://v2.tauri.app/develop/debug/linux-graphics/) docs recommend, applied via `gpu_compat::apply()` as the first statement of `run()` (`lib.rs`) — before `tauri::Builder::default()`, so it precedes any GTK/WebKit initialization.

The fix is **gated, not unconditional**: it only fires when both `/sys/module/nvidia` exists (the NVIDIA kernel module — proprietary or open, deliberately *not* matching `nouveau`, since this is an NVIDIA-driver bug and firing on nouveau would slow down machines that don't have it) and a Wayland session is detected (`WAYLAND_DISPLAY` set or `XDG_SESSION_TYPE=wayland`). Tauri's docs explicitly warn that an unconditional override "disables a faster path for everyone, including users on working setups" — the gate is what keeps every other combination (X11, Intel/AMD, macOS, Windows) bit-for-bit unaffected. `apply()` is a no-op on every non-Linux target.

Two vars are applied, cheapest first: `__NV_DISABLE_EXPLICIT_SYNC=1` (often fixes Error 71 with no performance cost) then `WEBKIT_DISABLE_DMABUF_RENDERER=1` (the stronger, user-verified fix — costs the faster DMA-BUF rendering path). `WEBKIT_DISABLE_COMPOSITING_MODE=1` — Tauri's third, most expensive option, for silent crash-on-resize — is deliberately **not** set; nothing in the reported symptom points to that failure mode. A variable already set by the user is never overwritten. `RESTY_DISABLE_GPU_WORKAROUND` (any non-empty value other than `0`) skips detection entirely, so an affected user can test whether a driver update has fixed things upstream, or rule the workaround out as the cause of an unrelated rendering complaint, without a rebuild. One `eprintln!` line names which variables were applied when the workaround fires, so it's visible in a bug report rather than invisible magic.

The core decision logic (`should_apply`, `is_opted_out`, `is_wayland`) is a pure, `cfg`-free function unit-tested on every platform (including macOS, where this was developed) — only the real environment-reading wrapper (`apply()`'s Linux body) is `#[cfg(target_os = "linux")]`. Because the pure items have no non-test caller off Linux, they each carry `#[cfg_attr(not(target_os = "linux"), allow(dead_code))]` with a comment — omitting it passes `npm run test:rust` but fails `npm run lint:rust` (`cargo clippy --all-targets -D warnings` builds the lib target too, where they're genuinely unreferenced off Linux). Don't "simplify" this by gating the whole module behind `#[cfg(target_os = "linux")]` — that would make the unit tests only run in CI, never on a non-Linux dev machine, which defeats the reason the pure/wrapper split exists.

