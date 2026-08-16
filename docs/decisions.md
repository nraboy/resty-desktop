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
- **The on-disk stored-size figure (`ResticStats.raw_size`) comes from a second `restic stats
  --mode raw-data --json` call, not a local filesystem walk (`du`/`walkdir`).** raw-data mode works
  identically for local *and* remote repos (S3/B2/REST/SFTP) with no filesystem access needed,
  reuses the existing `run_restic_blocking`/`RepoLocks` plumbing, and is fast (index load only, no
  tree walk) — a directory walk would only ever work for local repos and would need a new
  dependency-free traversal helper for something restic already reports. Accepted that raw-data
  mode counts blob data only, excluding the few MB of index/snapshot/key/lock files — immaterial
  against a repo measured in GB/TB. **The raw-data call's failure is deliberately non-fatal to the
  stats refresh as a whole** — it's logged, `raw_size` is left `None` for that cycle, and the
  refresh still succeeds and caches the (unaffected) restore-size figures. Making it fatal would
  turn an older-restic-binary or transient-remote-backend hiccup into a full "refresh failed" that
  blanks out numbers the user already had, with no way to recover them short of a lucky next
  refresh — worse than just not having the new number this cycle. See docs/restic.md.
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
- **A login launch shows a hidden window only when tray, auto-unlock, and launch-at-login are all
  three enabled; any other combination shows the window.** This used to be an unconditional "never
  launch hidden" — the app always started locked (`MasterKey` is in-memory only, no auto-unlock
  command existed), and a hidden launch would have parked a locked app in the tray running nothing
  until the user found and clicked the icon, advertising background activity the app couldn't
  actually perform yet. Revisiting it required first designing an auto-unlock story, which is a
  security decision, not a startup one — **that story is now the opt-in, default-off auto-unlock
  toggle** (see Security Architecture), which satisfies the original blocker. `lib.rs`'s `setup()`
  computes `start_hidden` via the standalone `should_start_hidden(from_autostart, tray_on,
  auto_unlock_on)` — pulled out of `setup()` and pinned by
  `should_start_hidden_requires_all_three` specifically so the "all three, not just auto-unlock"
  rule can't be quietly narrowed later — where `from_autostart` comes from a `--from-autostart`
  arg the `tauri_plugin_autostart::Builder` now passes (verified present in the pinned
  `auto-launch 0.5.0` on all three platforms: macOS LaunchAgent `ProgramArguments`, the Windows
  `Run` value, the Linux `Exec=` line). All three conditions are required, not just auto-unlock:
  without the tray there is no icon to bring the window back at all. **`App.tsx` shows the window
  on every auth state except `unlocked` — but only before the session's first unlock.** A
  `hasBeenUnlockedRef` (a ref, not state, so setting it doesn't itself re-trigger the effect)
  distinguishes a hidden launch that hasn't succeeded yet — where a failed auto-unlock
  (`denied`/`stale`) or the macOS post-update `updateNotice` prompt must always surface the
  window, so a hidden start can never end in a locked, invisible app with no obvious way in
  besides the tray — from a deliberate mid-session "Lock Now" fired from the tray's own
  unlocked-only menu item (see the tray entry below), which must leave an already-hidden window
  hidden rather than popping it back open. `setup()` also arms a one-shot
  20s watchdog (`MasterKey::is_locked()`, a boolean-only probe that never copies the key out of its
  zeroize-on-drop storage) that force-shows the window if a hidden start is still locked by then —
  covering the case where the frontend itself never loads far enough to call back into Rust.
  **On Linux this path can never trigger**: `keychain.rs`'s Linux build is a total no-op stub, so
  `auto_unlock` can never be `true` there — Linux keeps exactly the old always-visible behavior.
  Existing users' autostart entries predate `--from-autostart`; `setup()` re-registers the entry
  once (`app.autolaunch().enable()`, guarded on `is_enabled()` so it never creates an entry nor
  fights a Windows Task-Manager-disabled `Run` value) and records `autostart_args_migrated` in
  `app_settings` so it only runs once. A second autostart-triggered launch must not un-hide an
  already-hidden session: the single-instance handler ignores its own `--from-autostart` arg.
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
- **The tray icon is created in `setup()` (locked variant) whenever `tray_enabled` is on, not
  lazily after unlock.** Previously `activate_tray` was only ever called from `App.tsx` once
  `authState === "unlocked"`, so a locked app had no tray icon and closing the unlock-screen window
  quit the app outright even with the tray setting on. The locked-vs-unlocked menu differs: the
  locked variant carries a disabled, unclickable `Locked` header row and omits `Settings` and
  `Lock Now` entirely (both fire events — `menu:settings`/`menu:lock-app` — that only the unlocked
  React subtree listens for; on the lock screen they'd be dead clicks). `Lock Now` exists on the
  tray specifically because this change makes "unlocked and hidden in the tray indefinitely" a
  reachable state: on macOS the native menu bar (which already had its own `Lock Now`) is
  unreachable while the window is hidden under `ActivationPolicy::Accessory`, so without a tray
  equivalent there would be no way to lock such a session at all short of reopening the window.
  Its handler deliberately does not call `show_window` — see the `hasBeenUnlockedRef` guard in the
  hidden-launch entry above for the frontend half of keeping a Lock Now silent. The header row
  exists **in the menu, not just the tooltip**, because
  GNOME's AppIndicator extension routinely drops tray tooltips — the locked signal has to survive
  on the one platform where the tray is already weakest. `TrayState` accordingly tracks which
  variant is installed (`Tray { icon, unlocked, gen }`), and `lib.rs`'s close-to-tray handler no
  longer checks whether a tray exists at all — it hides on `tray_enabled` alone, in every auth state.
- **`activate_tray` short-circuits when the requested variant is already installed, and otherwise
  updates the existing icon in place with `set_menu`/`set_tooltip` rather than rebuilding it.**
  `App.tsx` calls it on every auth-state transition (not just once, on unlock), so both matter.
  The early-out (`guard.icon.is_some() && guard.unlocked == Some(unlocked)`) collapses a normal
  launch-then-unlock sequence to exactly one real update and incidentally absorbs React
  StrictMode's dev-only double-invoke. The in-place update is not just a Windows
  `NIM_DELETE`/`NIM_ADD`-flicker optimization — **rebuilding was an outright bug**: dropping the
  stored `TrayIcon` handle and calling `build_tray` again does not remove the old OS icon.
  `tauri::tray::TrayIcon` wraps a reference-counted `tray_icon::TrayIcon`
  (`Rc<RefCell<platform_impl::TrayIcon>>`), and `TrayIconBuilder::build` stores a *second* clone
  in Tauri's own resource table (`TrayIcon::register`, called from inside `build`) — so our
  stored handle was never the last reference. The result was a silent leak: the old (locked)
  icon stayed live in the menu bar, with its `on_menu_event` closure still registered, while a
  new (unlocked) icon appeared next to it — read by users as "the tray still says locked after
  unlocking," since the stale locked icon was indistinguishable from the new one. `build_tray`
  now returns `(TrayIcon, u32)` (the generation), stored once in `Tray::gen`; `build_tray_menu`
  is split out so `activate_tray` can rebuild just the `Menu` for the *same* generation and hand
  it to the existing icon via `set_menu`, while the icon's `on_menu_event` closure recomputes its
  ids from the same captured `gen` at click time — so it keeps matching whichever menu is
  currently installed. Don't reintroduce a rebuild-to-swap-variants pattern for this tray; it
  will silently reproduce the leak. `deactivate_tray` is the only place that must actually free
  the OS icon, and does so correctly by calling `AppHandle::remove_tray_by_id` (which takes the
  resource-table clone out) before dropping the last reference on the main thread — seeing
  `set_visible(false)` plus a Windows-only `mem::forget` there again would mean this bug came back.
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
- **Linux file dialogs use `tauri-plugin-dialog`'s `xdg-portal` feature
  (`default-features = false, features = ["xdg-portal"]`), not the plugin's default `gtk3`
  feature.** Reported by a user: the default GTK file chooser looks and behaves out of place on
  non-GNOME desktops (KDE, etc.) since it ignores the desktop's native picker. `rfd` (the crate
  `tauri-plugin-dialog` wraps) compiles exactly one Linux backend at build time and prefers
  `gtk3` whenever that feature is enabled *anywhere* in the dependency graph — so `gtk3` must
  stay off, not merely be left non-default, or the portal silently reverts to GTK. This has no
  effect on macOS or Windows: `gtk3` only gates Linux/BSD-target deps in `rfd`, and the plugin
  already pins `rfd` itself with `default-features = false, features = ["common-controls-v6"]`,
  so Windows already gets its native picker regardless of this flag. No `cfg(target_os = ...)`
  gating was needed for that reason.
  There is **no GTK fallback** if the portal is unavailable: `rfd`'s portal backend returns
  `None` when `xdg-desktop-portal` (or a desktop-specific backend) isn't reachable over DBus,
  and every call site in this app treats `None` identically to a user cancel — so a missing
  portal makes every Browse button in the app go silently inert, not show an error. To close
  that for the common case, `tauri.conf.json`'s `bundle.linux.deb.depends` and `rpm.depends`
  both list `xdg-desktop-portal` so a package install can't land without it (the base package
  itself recommends a desktop-specific backend, so backends aren't listed explicitly). A
  **startup DBus probe** for `org.freedesktop.portal.Desktop`, to warn proactively when no
  portal is reachable, was considered and deliberately **not** added — the remaining exposure
  after the packaging fix is tarball/self-built installs on bare window managers, a narrow
  enough slice that the same population is already likely to have a portal from using
  Flatpak/screen-sharing/etc., and doesn't justify a new dependency and a new banner. Don't
  add that probe without discussing the trade-off first.
  A second, minor, accepted behavior change: `rfd`'s portal backend drops any picked location
  that isn't a `file://` URI (`uri.to_file_path().ok()`), so selecting a network location
  (smb://, mtp://) from the portal's sidebar returns nothing, where the old GTK chooser would
  have resolved it to a local gvfs mount path. Backing up a gvfs mount with restic is a niche
  enough case not to block on.
- **Windows and Linux install no native menu bar at all; only macOS does.** The menu is
  still built and `MenuState` managed on every platform, but `setup()` drops it on
  Windows/Linux (`#[cfg]` gate around `app.set_menu(menu)` in `lib.rs`) — extending what
  Linux already did for GTK dark-theme-unreadability reasons. History: a user first
  reported that on Windows the window title bar, the app submenu's label, and the sidebar
  logo stacked three near-identical "Resty Desktop" strings within a few dozen pixels
  (unlike macOS, the app submenu renders inside the window's own menu bar). That was
  initially fixed by folding the app submenu into `File` with an "Exit" item — which worked
  but cost Windows-only `MenuState` fields, a Windows-only re-pinning branch in
  `set_menu_auth_state`, and cfg forks throughout menu assembly. Hiding the menu entirely
  replaced the workaround: it deletes the whole duplicate-label problem class *and* the
  fold machinery, and matches Windows norms (browsers, VS Code, etc. ship without visible
  menu bars). Nothing of value was lost: every menu feature is reachable in the sidebar,
  pages, or tray; the only accelerator anywhere in the menu was Ctrl+Q (window close /
  Alt+F4 still quit); and the predefined Edit items' Cut/Copy/Paste shortcuts are handled
  natively by WebView2 without a menu. "Lock Now" gained a tray-independent home — a Lock
  item at the bottom of the sidebar's nav list (`Sidebar`'s `onLock` prop, shared `handleLock` in
  `App.tsx` with the `menu:lock-app` event listener) — because with the menu gone and the
  tray setting off there would otherwise be no way to lock a session short of quitting.
  The locked-state "Reset Application" path is covered by the lock screen's existing
  "Forgot your password?" modal. macOS keeps its system menu bar (it lives in the OS menu
  bar, away from the window — no label-stacking problem, and it's where Mac users expect
  it); don't "complete" this by hiding it there, and don't reintroduce a Windows menu bar
  or the File-fold — both were the reported bug.
- **`@floating-ui/react` was added as a runtime dependency for `components/Tooltip.tsx`,
  rather than hand-rolling a tooltip the way `ContextMenu.tsx` hand-rolls its portal +
  fixed-position + overflow-nudge in ~70 lines.** This looks like it should've stayed
  dependency-free given the app's otherwise minimal (six-runtime-dep) posture, and it was
  seriously considered — `ContextMenu.tsx` already proves the core mechanics (portal to
  `document.body`, measure via `getBoundingClientRect`, nudge onto screen) work fine
  hand-rolled at this app's scale. The reason a tooltip doesn't get the same treatment: a
  context menu closes on any mousedown, so it never needs to track its anchor after opening.
  A tooltip anchored to a row inside a scrolling table (SnapshotsPage's Paths column,
  ActivityPanel's Upcoming Tasks list) goes stale the instant the page scrolls — it detaches
  from its trigger and floats over unrelated rows — unless something repositions it live.
  Floating UI's `autoUpdate` is exactly that, plus hover-intent grace between adjacent
  triggers and `role="tooltip"`/`aria-describedby` wiring, all of which a hand-rolled version
  would eventually reinvent, badly, as bug reports. The dependency cost is low in this app
  specifically: Tauri bundles the JS locally (no network-fetch cost to the extra ~20kb), the
  package is headless (it positions; it doesn't paint, so it touches none of the
  CSS-custom-property theming), and its transitive footprint is small: the rest of
  `@floating-ui/*` (its own first-party packages) plus one non-`@floating-ui` package,
  `tabbable` (focus-trapping support for `FloatingFocusManager`/roving-focus interactions),
  confirmed via `npm ls @floating-ui/react tabbable` at the time it was added — recheck
  before assuming that's still true. Don't hand-roll a second tooltip
  implementation "to avoid the dependency" — extend `Tooltip.tsx` instead.
- **Not every `title=` attribute was converted to `Tooltip.tsx` when it was added — most
  stayed on native `title`, and that's deliberate, not partial completion.** The rule:
  short strings that just name a control (icon-button labels — `ActionButton.tsx`, most of
  `ActivityPanel.tsx`, disabled-state explanations like "This repository is read-only") stay
  on native `title`; the ~1s delay and lack of styling don't matter for a one-line label, and
  it's the semantically-correct, zero-code, screen-reader-friendly choice for that case. Only
  hovers that carry real *content* — multi-line lists, monospaced text, a derived percentage,
  anything a plain unstyled/unformatted box renders badly — convert to `Tooltip`. Don't
  "finish the job" by converting every remaining `title=` in the app; audit each one against
  that rule first. See `Tooltip.tsx`'s entry in docs/frontend.md for the converted-site list.


## Linux GPU Compatibility

## Linux GPU Compatibility

`src-tauri/src/gpu_compat.rs` works around a known WebKitGTK/NVIDIA/Wayland crash (`Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.`, reproduced by users on Fedora + NVIDIA + Wayland) by setting the same env vars Tauri's own [Linux Graphics Issues](https://v2.tauri.app/develop/debug/linux-graphics/) docs recommend, applied via `gpu_compat::apply()` as the first statement of `run()` (`lib.rs`) — before `tauri::Builder::default()`, so it precedes any GTK/WebKit initialization.

The fix is **gated, not unconditional**: it only fires when both `/sys/module/nvidia` exists (the NVIDIA kernel module — proprietary or open, deliberately *not* matching `nouveau`, since this is an NVIDIA-driver bug and firing on nouveau would slow down machines that don't have it) and a Wayland session is detected (`WAYLAND_DISPLAY` set or `XDG_SESSION_TYPE=wayland`). Tauri's docs explicitly warn that an unconditional override "disables a faster path for everyone, including users on working setups" — the gate is what keeps every other combination (X11, Intel/AMD, macOS, Windows) bit-for-bit unaffected. `apply()` is a no-op on every non-Linux target.

Two vars are applied, cheapest first: `__NV_DISABLE_EXPLICIT_SYNC=1` (often fixes Error 71 with no performance cost) then `WEBKIT_DISABLE_DMABUF_RENDERER=1` (the stronger, user-verified fix — costs the faster DMA-BUF rendering path). `WEBKIT_DISABLE_COMPOSITING_MODE=1` — Tauri's third, most expensive option, for silent crash-on-resize — is deliberately **not** set; nothing in the reported symptom points to that failure mode. A variable already set by the user is never overwritten. `RESTY_DISABLE_GPU_WORKAROUND` (any non-empty value other than `0`) skips detection entirely, so an affected user can test whether a driver update has fixed things upstream, or rule the workaround out as the cause of an unrelated rendering complaint, without a rebuild. One `eprintln!` line names which variables were applied when the workaround fires, so it's visible in a bug report rather than invisible magic.

The core decision logic (`should_apply`, `is_opted_out`, `is_wayland`) is a pure, `cfg`-free function unit-tested on every platform (including macOS, where this was developed) — only the real environment-reading wrapper (`apply()`'s Linux body) is `#[cfg(target_os = "linux")]`. Because the pure items have no non-test caller off Linux, they each carry `#[cfg_attr(not(target_os = "linux"), allow(dead_code))]` with a comment — omitting it passes `npm run test:rust` but fails `npm run lint:rust` (`cargo clippy --all-targets -D warnings` builds the lib target too, where they're genuinely unreferenced off Linux). Don't "simplify" this by gating the whole module behind `#[cfg(target_os = "linux")]` — that would make the unit tests only run in CI, never on a non-Linux dev machine, which defeats the reason the pure/wrapper split exists.

