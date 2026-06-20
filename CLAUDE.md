# Resty Desktop

A cross-platform desktop client for the Restic CLI backup tool.

## Technical Details and Features

- Uses Tauri v2 as the cross-platform desktop framework.
- Acts as a wrapper to the already established Restic CLI application, installed separately.
- Should be able to create repositories, add folders to snapshots, view snapshots, view the files within snapshots, tag snapshots, delete snapshots, etc.

## Stack

| Layer | Choice |
|---|---|
| Desktop shell | Tauri v2 |
| Frontend | React 19 + TypeScript |
| Styling | Tailwind CSS v3 + CSS custom properties for theming |
| Build tool | Vite |
| State management | URL-based nav (no global store) |
| Routing | React Router v6 |
| Rust backend | Tauri v2 `#[tauri::command]` |
| Settings persistence | SQLite (`app_data.db`) via `AppDb` |
| File picker | `tauri-plugin-dialog` |
| Shell plugin | `tauri-plugin-shell` (registered but not exposed to frontend) |
| Memory safety | `zeroize` crate — `MasterKey` and `FullRepository` zeroize sensitive bytes on drop/replace |
| Notifications | `tauri-plugin-notification` — shown on backup success/failure |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Restic integration | `std::process::Command` with `--json` flag |

## Project Structure

```
src/
  App.tsx                     # Router + layout shell; handles auth state machine (loading/setup/locked/unlocked);
                              #   on unlock, calls getResticVersion and shows a dismissable warning banner if the
                              #   detected version is below MIN_RESTIC_MAJOR.MIN_RESTIC_MINOR (from config.ts);
                              #   routes are wrapped in an ErrorBoundary class component that catches unhandled render
                              #   errors and shows a "Something went wrong / Try again" fallback instead of a white screen
  main.tsx                    # React entry point; suppresses the default browser/WebKit context menu globally
                              #   via document.addEventListener("contextmenu", e => e.preventDefault())
  index.css                   # Tailwind directives + global styles
  components/
    Button.tsx                # Reusable button (primary/secondary/danger/ghost variants)
    ContextMenu.tsx           # Right-click context menu rendered via React portal into document.body;
                              #   accepts x/y position + ContextMenuItemDef[] (label/onClick/variant/disabled or separator);
                              #   auto-nudges onto screen if it would overflow an edge; closes on click-outside or Escape
    EmptyState.tsx            # Empty list placeholder
    Input.tsx                 # Labeled input with error state
    Modal.tsx                 # Overlay modal dialog
    Sidebar.tsx               # Left nav with app icon + "Resty Desktop" title; active repo indicator
  lib/
    types.ts                  # Shared TS types: Repository, Snapshot, FileEntry, ResticStats, SnapshotStats, CheckResult,
                              #   BackupHistoryEntry, BackupProgress, RestoreProgress, RetentionPolicy, BackupPlan; isRemoteRepo() helper;
                              #   BackupPlan includes optional limitUpload and limitDownload (KiB/s); 0 and undefined both mean unlimited;
                              #   DiffEntry { path, change } and DiffResult { entries, totalAdded, totalRemoved, totalModified, truncated }
    invoke.ts                 # Typed wrappers over tauri invoke()
    format.ts                 # Shared display formatters: formatBytes, formatSize, formatDate, formatTimestamp,
                              #   formatDuration (fractional param for sub-minute precision); used by all pages
    config.ts                 # App-level constants: MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR — bump to change minimum
                              #   supported restic version; consumed by App.tsx version warning banner
    theme.tsx                 # React context-based theming: exports ThemeProvider (wraps the app in App.tsx)
                              #   and useTheme() hook (returns { theme, setTheme }); persists selection to
                              #   localStorage under key "resty-theme"; applies "light"/"dark"/"system" class
                              #   to <html> via applyClass(); listens to prefers-color-scheme media query
                              #   when theme === "system" to re-apply on OS theme change
  pages/
    AuthPage.tsx              # Master password setup (first launch) and unlock screen; shown before main UI
    RepositoriesPage.tsx      # Add/open/delete repos; triggers restic init for new repos; supports remote URLs (S3, SFTP, etc.);
                              #   per-row refresh stats button shown for all repos (not just remote); "Refresh Stats" header button
                              #   refreshes repos in parallel via Promise.allSettled; remote repos are excluded unless remote_auto_refresh=true;
                              #   refreshingAll state disables per-row buttons during bulk refresh;
                              #   mirror repository modal: copies all snapshots from one repo to another with indeterminate progress bar,
                              #   elapsed timer, cancellation support, and completion/cancellation confirmation UI;
                              #   edit repository modal: allows changing name, path, and password; path shows Browse button for local repos
                              #   and a text input for remote URLs; password pre-loaded (masked) via get_repo_password;
                              #   Test Connection button validates the current path+password before saving;
                              #   right-click context menu on each repo row: Open Snapshots, Refresh Stats, Check Repository,
                              #   Edit, Mirror, Prune, Delete — Check Repository runs check_repo and shows result in a modal
                              #   (spinner while running, then success/error UI matching SnapshotsPage); Check and Prune only appear in
                              #   the context menu, not as row buttons; Prune modal has confirmation → indeterminate progress + elapsed
                              #   timer + Cancel → done/cancelled states, uses PruneHandle + cancel_prune for cancellation
    SnapshotsPage.tsx         # Table of snapshots; inline tag editor; delete with prune option; stale-while-revalidate cache pattern; on-demand repo check;
                              #   amber banner shown for remote repos when remote_auto_refresh=false, prompting manual Refresh or enabling auto-refresh in Settings;
                              #   full-snapshot restore modal with streaming progress bar (restore:progress events); restore
                              #   target dir pre-filled from get_restore_path setting (user can override per-restore via Browse);
                              #   per-snapshot copy to another repo with cancellation support;
                              #   snapshots and loading state are cleared immediately on repoId change to prevent stale data flash when navigating between repos;
                              #   paginated at PAGE_SIZE=10 rows per page; pagination applies to the filtered set; page resets to 0 on filter change or repo
                              #   navigation; page is clamped to last valid page when filtered set shrinks (e.g. after delete) to avoid empty-page trap;
                              #   right-click context menu on each snapshot row: Browse Files, Restore…, Copy to Repository…, Add Tag…,
                              #   Compare with…, Snapshot Stats, Delete — Snapshot Stats calls get_snapshot_stats (restic stats --json <id>) and shows
                              #   total size + file count in a modal (spinner while running, note that size includes shared data);
                              #   Compare with… opens a modal to select a second snapshot and navigates to DiffPage; always diffs older→newer
                              #   regardless of which snapshot was right-clicked (timestamps compared to determine order)
    BrowsePage.tsx            # File tree navigation inside a snapshot; per-entry restore via icon button (download glyph)
                              #   and right-click context menu ("Restore…" item); native directory picker (Browse button),
                              #   target pre-filled from get_restore_path setting; restore modal includes
                              #   "Restore file/folder only" checkbox (default checked) — when checked, strips the original
                              #   path structure so the item lands directly in the target dir instead of nested under its
                              #   full original path; breadcrumb nav;
                              #   multi-select restore: "Select Multiple" toggle shows per-row checkboxes + select-all header
                              #   checkbox; amber banner counts selected items and shows "Restore selected" button; multi-restore
                              #   modal restores each selected path sequentially via restorePath, showing a determinate progress
                              #   bar (current/total) and current path; selection clears on breadcrumb navigation or directory
                              #   entry; "Cancel select" exits selection mode and clears selection;
                              #   inline tag management (add/remove tags on the snapshot directly from the browse view)
    DiffPage.tsx              # Snapshot diff viewer at route /snapshots/:repoId/diff/:snapshotA/:snapshotB;
                              #   calls diff_snapshots on mount and builds a client-side tree from the flat entry list;
                              #   summary bar shows colored counts (green=added, red=removed, amber=modified);
                              #   amber truncation warning shown when result.truncated is true (>500 changes);
                              #   tree navigation via breadcrumb; directories show aggregated change type
                              #   (all-same→that type, mixed→"mixed" in gray); right-click any entry to Restore —
                              #   "removed" files restore from snapshotA (older), all others from snapshotB (newer);
                              #   restore modal pre-filled from get_restore_path, uses restorePath invoke (same as BrowsePage);
                              #   restore modal includes "Restore file/folder only" checkbox (default checked, same behavior as BrowsePage)
    BackupPlansPage.tsx       # List saved backup plans; run a plan immediately; delete plans;
                              #   backup modal with streaming progress bar (backup:progress events), cancellation support
                              #   (cancel_backup), and completion/error confirmation UI;
                              #   retention is automatically applied after every successful backup run — if the plan has a
                              #   retention policy, forgetByPlan is called immediately after runBackup resolves (applyingRetention
                              #   state shown in the modal during this phase); no separate user action needed;
                              #   "Apply Retention" funnel button shown per-plan when a retention policy with at least one
                              #   keep rule is configured; opens a modal that runs forget_by_plan standalone (no backup);
                              #   row icons use 24px outline stroke style matching RepositoriesPage;
                              #   right-click context menu on each plan row: Edit Plan, Run Backup, Apply Retention Rules
                              #   (only shown when plan has at least one keep rule), Delete
    BackupPlanEditPage.tsx    # Create/edit a backup plan (name, repo, paths, tags, excludes, retention policy, bandwidth limits); planId="new" for creation;
                              #   exclude patterns use tabbed Simple (tag list) / Expert (freeform textarea) UI;
                              #   Simple tab includes EXCLUDE_SUGGESTIONS presets (Development assets: node_modules/.git/build output/etc.;
                              #   System files: .DS_Store/Thumbs.db; Log files: *.log variants) that can be one-click added to the pattern list;
                              #   bandwidth limits section: optional upload/download caps in KiB/s (limitUpload, limitDownload); blank or 0 = unlimited;
                              #   note shown that limits only affect remote repos — no effect on local filesystem repos
    SchedulesPage.tsx         # List scheduled backups; toggle enabled/disabled; delete; run immediately;
                              #   shows an amber warning banner when tray_enabled=false, with a link to Settings,
                              #   because schedules cannot run while the window is closed without the tray
    ScheduleEditPage.tsx      # Create/edit a schedule (name, cron expression, backup plans to run); scheduleId="new" for creation
    LogsPage.tsx              # Persistent backup history log; shows date, plan, repo, duration, file counts, bytes added, snapshot ID; expandable error rows;
                              #   paginated at PAGE_SIZE=10 rows per page with Previous/Next controls and a "Page X of Y · N total entries" counter
    SettingsPage.tsx          # Appearance (theme selector) shown first; Toggles section with system tray toggle
                              #   (get/set_tray_enabled + activate_tray/deactivate_tray on change) and remote auto-refresh toggle
                              #   (get/set_remote_auto_refresh — default false; when true, snapshot lists and repo stats for remote
                              #   repos refresh automatically on page load; amber warning shown when enabled); Restic binary path
                              #   override; shows detected restic version below path input; install instructions section
                              #   hidden when restic is found; global backup compression selector
                              #   (off/fastest/auto/better/max) persisted to app_settings; default restore path
                              #   (get/set_restore_path) with Browse button — pre-fills target dir in all restore modals,
                              #   defaults to <home>/restores on first use (computed via Tauri's app.path().home_dir());
                              #   prune all repositories: runs restic prune on every repo sequentially with a modal showing
                              #   progress bar, elapsed timer, per-repo name, and cancellation support (prune:progress events)

src-tauri/
  Cargo.toml
  tauri.conf.json
  src/
    main.rs                   # Calls restic_gui_lib::run()
    lib.rs                    # Tauri builder; registers all commands; opens app_data.db, initialises schema, manages AppDb + MasterKey as Tauri state;
                              #   manages CopyHandle, MirrorHandle, BackupHandle, PruneHandle as Tauri state;
                              #   calls recalculate_overdue_schedules at startup to skip missed schedule runs;
                              #   builds native menu bar (MenuState) with "Resty Desktop" and "File" submenus; items are auth-aware
                              #   (Settings/New Repository/New Backup Plan shown when unlocked; Reset Application shown when locked);
                              #   menu events emitted as menu:new-repository, menu:new-backup-plan, menu:settings, menu:reset-app Tauri events;
                              #   set_menu_auth_state command called from frontend after auth transitions;
                              #   system tray (TrayState) is created lazily after unlock via activate_tray command — no tray before unlock,
                              #   so closing the window pre-unlock exits the app normally; after unlock, window close hides to tray if
                              #   tray_enabled=true (checked via AppDb), otherwise exits; activate_tray always recreates fresh using a
                              #   TRAY_GEN atomic counter for unique menu item IDs (avoids Tauri global-registry collisions on re-creation);
                              #   deactivate_tray: calls set_visible(false) on all platforms for immediate visual removal,
                              #   then on Windows uses std::mem::forget to skip Drop (set_visible = NIM_DELETE on Windows,
                              #   so Drop would issue a second NIM_DELETE and log an error); on macOS/Linux Drop runs
                              #   normally after set_visible since hide and delete are separate operations there;
                              #   macOS uses icons/tray-icon.png
                              #   (black template icon), other platforms use icons/32x32.png (colorful); show_window helper restores
                              #   the window and macOS activation policy; RunEvent::Reopen handles macOS dock-click while window is hidden —
                              #   gated with #[cfg(target_os = "macos")] because the variant does not exist on Windows/Linux
    commands/
      mod.rs                  # shared get_restic_path() helper used by all command modules;
                              #   NoConsole trait adds no_console() (suppresses Windows console window) and augment_path()
                              #   (prepends /opt/homebrew/bin, /usr/local/bin, and other common dirs to the child process
                              #   PATH on macOS/Linux so bare binary names like "restic" and "rclone" resolve correctly
                              #   when the app is launched from Finder/DMG where the inherited PATH is minimal)
      auth.rs                 # is_app_setup, setup_master_password, unlock_app, lock_app,
                              #   change_master_password, reset_app;
                              #   unlock_app runs restic unlock on all repos in the background to clear stale locks from crashes
      crypto.rs               # Argon2id key derivation, AES-GCM encrypt/decrypt helpers
      repo.rs                 # list_repos, add_repo, remove_repo, init_repo, rename_repo,
                              #   update_repo_path, get_repo_password (decrypts + returns stored password),
                              #   update_repo_password (re-encrypts new password with master key),
                              #   test_repo_connection, get_repo_stats, refresh_repo_stats, get/set_restic_path,
                              #   get_restic_version, check_repo, get_compression, set_compression,
                              #   get_restore_path / set_restore_path (read/write "restore_path" key in app_settings;
                              #   get_restore_path computes <home>/restores via app.path().home_dir() on first call and
                              #   stores it so subsequent calls are fast and the value is editable in Settings),
                              #   get_tray_enabled / set_tray_enabled (read/write "tray_enabled" key in app_settings),
                              #   get_remote_auto_refresh / set_remote_auto_refresh (read/write "remote_auto_refresh" key in app_settings; default "false"),
                              #   prune_all_repos (runs restic prune on every repo sequentially, emits prune:progress events,
                              #   cancellable via PruneHandle), prune_repo (runs restic prune on a single repo, same
                              #   PruneHandle + cancel_prune cancellation pattern), cancel_prune;
                              #   set_restic_path validates non-empty and checks file existence for absolute paths before saving;
                              #   run_restic_with_path uses String::from_utf8 (strict) for stdout and from_utf8_lossy for stderr
      snapshot.rs             # list_snapshots, refresh_snapshots, delete_snapshot, tag_snapshot,
                              #   get_snapshot_stats (runs restic stats --json <id>, returns SnapshotStats { totalSize, totalFileCount }),
                              #   execute_backup (pub async helper shared by run_backup, run_schedule_now, scheduler.rs),
                              #   run_backup (delegates to execute_backup), cancel_backup (kills BackupHandle child),
                              #   forget_by_plan (when filtering by tags, passes --group-by tags so retention is applied
                              #   per tag-group across all hosts/paths — avoids per-host grouping surprises), unlock_repo,
                              #   copy_snapshot (streams to dest repo with cancellation), cancel_copy;
                              #   mirror_repo (copies all snapshots src→dest via restic copy, cancellable), cancel_mirror;
                              #   both cancel_copy, cancel_mirror, and cancel_backup run restic unlock after SIGKILL to clear stale locks;
                              #   validate_snapshot_id() guards delete_snapshot, tag_snapshot, copy_snapshot, get_snapshot_stats,
                              #   diff_snapshots — rejects anything outside 8–64 lowercase hex characters before any crypto or restic work;
                              #   diff_snapshots runs restic diff <snapshotA> <snapshotB> (plain text output, no --json),
                              #   parses lines by prefix (+  /−  /M  /T  ), counts totals for all lines, stores up to
                              #   DIFF_ENTRY_LIMIT=500 entries with path.trim() to handle whitespace variations;
                              #   returns DiffResult { entries, totalAdded, totalRemoved, totalModified, truncated }
      browse.rs               # list_files, restore_path, restore_snapshot;
                              #   restore_path accepts strip_leading_path: bool — when true, after restic finishes it moves
                              #   the restored item from <target>/<original/full/path> to <target>/<basename> and removes
                              #   the now-empty ancestor directories; source and dest are always under target_dir so
                              #   std::fs::rename never crosses filesystems; on Windows, restic exits non-zero when it
                              #   cannot apply platform-specific extended attributes (e.g. macOS EAs) — restore_path
                              #   suppresses errors whose every stderr line is EA-related (set EA failed / extended attribute)
                              #   because the file is fully restored despite the metadata failure; strip_leading_path uses
                              #   an .exists() guard so it runs correctly whether restic exited cleanly or with EA-only errors
      backup_plan.rs          # list_backup_plans, save_backup_plan, remove_backup_plan; plans stored in SQLite;
                              #   list_backup_plans returns plans sorted alphabetically by name (ORDER BY name COLLATE NOCASE)
      schedule.rs             # list_schedules, save_schedule, remove_schedule, toggle_schedule,
                              #   run_schedule_now (accepts BackupHandle, calls execute_backup for each plan in the schedule),
                              #   describe_cron_expr; cron helpers next_fire_time + describe_cron
      cache.rs                # AppDb (unified SQLite state); MasterKey (in-memory, zeroizes key bytes on set/clear);
                              #   FullRepository derives ZeroizeOnDrop so password is wiped from memory when dropped;
                              #   list_backup_history returns up to 1000 rows (LIMIT 1000), ordered newest-first;
                              #   CopyHandle (in-memory, for cancel);
                              #   MirrorHandle (in-memory, for mirror cancellation);
                              #   BackupHandle (in-memory, same Arc<Mutex<Child>> + AtomicBool pattern, for backup cancellation);
                              #   PruneHandle (in-memory, same Arc<Mutex<Child>> + AtomicBool pattern, for prune cancellation);
                              #   AppDb::recalculate_overdue_schedules — advances overdue next_run_at to next future fire time at startup (skips missed backups);
                              #   AppDb helpers: rename_repo, update_repo_path, update_repo_password (UPDATE queries for individual repo fields);
                              #   Repository, FullRepository, BackupPlan, RetentionPolicy, BackupHistoryEntry,
                              #   Schedule types; clear_browse_cache, list_backup_history commands
  scheduler.rs                # Background tokio task (60s tick) that calls execute_backup for due schedules;
                              #   acquires BackupHandle state; skips silently when app is locked;
                              #   updates last_run_at and next_run_at after each run;
                              #   guards against overlapping ticks with an AtomicBool running flag
```

## Routes

| Path | Page |
|---|---|
| `/` | RepositoriesPage |
| `/snapshots/:repoId` | SnapshotsPage |
| `/snapshots/:repoId/:snapshotId/browse` | BrowsePage |
| `/snapshots/:repoId/diff/:snapshotA/:snapshotB` | DiffPage |
| `/backup-plans` | BackupPlansPage |
| `/backup-plans/:planId` | BackupPlanEditPage (`planId="new"` for creation) |
| `/schedules` | SchedulesPage |
| `/schedules/:scheduleId` | ScheduleEditPage (`scheduleId="new"` for creation) |
| `/logs` | LogsPage |
| `/settings` | SettingsPage |

## Restic Integration

- Restic binary path is user-configurable; defaults to `restic` on `$PATH`.
- All commands set both `RESTIC_REPOSITORY` and `RESTIC_PASSWORD` env vars — never pass either in process args.
- Structured output parsed via `restic --json`; `serde_json` deserializes responses into typed Rust structs.
- `restic ls --json` outputs NDJSON (one JSON object per line); the first line is a snapshot summary and is skipped; subsequent lines are `FileEntry` objects filtered to direct children only.
- `run_backup` delegates to `execute_backup` (a shared `pub async fn` also used by `run_schedule_now` and `scheduler.rs`). `execute_backup` streams NDJSON from restic stdout line-by-line; `status` lines are parsed and emitted as `backup:progress` Tauri events (consumed by the frontend progress bar); the final `summary` line is captured and returned (all other lines are discarded as they arrive — not buffered). On completion, fires a system notification (success or failure) via `tauri-plugin-notification` and writes a row to `backup_history`. A `BackupHandle` state (same `Arc<Mutex<Option<Child>>>` + `AtomicBool` pattern as `CopyHandle`) allows `cancel_backup` to kill the child process mid-run; after cancel-kill, `restic unlock` is called on the repo to clear stale locks. `execute_backup` reads the `compression` setting from `app_settings` (default `auto`) and passes it as `RESTIC_COMPRESSION` env var to the restic process. `execute_backup` also accepts `limit_upload: Option<u32>` and `limit_download: Option<u32>` (KiB/s); values of `Some(0)` are treated as `None` — the `--limit-upload`/`--limit-download` flags are only passed to restic when the value is non-zero; only meaningful for remote repos.
- `check_repo` runs `restic check --json`; progress/status lines go to stderr (ignored), only the summary lands on stdout. Duration is measured via `std::time::Instant` since the summary message contains no timing field. Returns `CheckResult { success, errors, duration_seconds }`.
- `restore_snapshot` streams `restic restore <id> --target <dir> --json` stdout line-by-line; `status` lines are parsed and emitted as `restore:progress` Tauri events (consumed by the frontend progress bar in the restore modal). Stderr is drained on a background thread and surfaced as the error message on non-zero exit.
- `copy_snapshot` runs `restic copy --from-repo <src> <snapshot_id>` against the destination repo; streams stdout and surfaces errors via stderr. A `CopyHandle` Tauri state (Arc<Mutex<Option<Child>>> + AtomicBool cancelled) allows `cancel_copy` to kill the child process mid-run. After a cancel-kill, `restic unlock` is called on both repos to clear stale locks.
- `mirror_repo` runs `restic copy` (no specific snapshot ID) with `RESTIC_FROM_REPOSITORY`/`RESTIC_FROM_PASSWORD` env vars to copy all snapshots from src to dest, skipping those already present. A `MirrorHandle` state (same pattern as `CopyHandle`) allows `cancel_mirror`. Also runs `restic unlock` on both repos after cancel.
- `prune_all_repos` runs `restic prune` on each repo sequentially, emitting `prune:progress` events (consumed by the SettingsPage modal). A `PruneHandle` state (same `Arc<Mutex<Option<Child>>>` + `AtomicBool` pattern as `BackupHandle`) allows `cancel_prune` to kill the child mid-run; after a cancel-kill, `restic unlock` is called on the affected repo to clear stale locks.
- `prune_repo` runs `restic prune` on a single repo (triggered from the RepositoriesPage right-click context menu); reuses the same `PruneHandle` and `cancel_prune` command. No progress events — the frontend shows an indeterminate progress bar with elapsed timer.
- `get_snapshot_stats` runs `restic stats --json <snapshot_id>` for a single snapshot; returns `SnapshotStats { totalSize, totalFileCount }`. Guarded by `validate_snapshot_id`. Note: size includes all data referenced by the snapshot, including blobs shared with other snapshots.
- `diff_snapshots` runs `restic diff <snapshotA> <snapshotB>` (no `--json` support); output is plain text with lines prefixed by `+  ` (added), `-  ` (removed), `M  ` or `T  ` (modified). Both IDs are guarded by `validate_snapshot_id`. Paths are extracted with `.trim()` to handle whitespace variations in restic output. Totals are counted from all lines; entries are capped at `DIFF_ENTRY_LIMIT=500` with a `truncated` flag when the cap is hit. The frontend (DiffPage) always navigates with the older snapshot as `snapshotA` and newer as `snapshotB` so `+` consistently means "added in the newer snapshot."
- `unlock_app` runs `restic unlock` on all repos in the background immediately after the master password is verified, clearing any stale locks left by a previous crash or force-quit.
- `get_restic_version` runs `restic version` and returns the trimmed stdout string (e.g. `restic 0.18.1 compiled with go1.25.1 on darwin/arm64`). Used by `SettingsPage` to verify the configured binary path is valid.
- Repos, backup plans, schedules, and app settings are stored in SQLite (`app_data.db`) via `AppDb`. Repo passwords are AES-GCM encrypted with the master key before storage.
- Schedules reference one or more backup plan IDs and a 5-field cron expression. The `scheduler.rs` background task polls every 60 seconds, finds due schedules via `list_due_schedules`, runs each referenced plan via `execute_backup`, then updates `last_run_at`/`next_run_at`. Silently skips when the app is locked.

## Security Architecture

- App requires a **master password** set on first launch (`setup_master_password`). Subsequent launches call `unlock_app` to load the key into memory.
- Master password is never stored. Instead: Argon2id derives a 32-byte key from password + random salt; AES-GCM encrypts a known verification plaintext; the salt + nonce + ciphertext are stored in the `master_key` table.
- All repo passwords are encrypted with the master key (AES-GCM) and stored in the `repositories` table. They are decrypted on-demand via `db.get_full_repo(&repo_id, &key)`.
- `MasterKey` is an in-memory `Mutex<Option<[u8; 32]>>` managed as `tauri::State`. It is `None` when locked; any command that calls restic will fail with "App is locked" until `unlock_app` succeeds. Key bytes are zeroized (via the `zeroize` crate) when replaced or cleared.
- `change_master_password` re-derives a new key and re-encrypts all repo passwords atomically in a SQLite transaction.
- `reset_app` wipes all SQLite tables and clears the in-memory key, returning to first-launch state.
- On first `setup_master_password`, any existing `settings.json` data (repos, backup plans, restic path) is migrated into SQLite and encrypted.

## Persistence Layer

- Single SQLite database (`app_data.db`) in the Tauri app data directory, opened at startup and managed as `tauri::State<AppDb>`.
- Tables: `master_key`, `repositories` (encrypted passwords), `backup_plans`, `schedules`, `app_settings`, `snapshots_cache`, `browse_cache`, `repo_stats_cache`, `backup_history`.
- All reads/writes go through `AppDb`. `tauri-plugin-store` has been removed entirely (legacy `settings.json` migration code was dropped).

## Caching Layer

- Three cache tables in `app_data.db`: `snapshots_cache` (per-repo snapshot list), `browse_cache` (per-snapshot directory listings keyed by path), `repo_stats_cache` (per-repo stats).
- `list_snapshots` — returns from cache only (fast, no restic call). `refresh_snapshots` — calls restic and updates cache.
- `SnapshotsPage` uses a stale-while-revalidate pattern: serve cache immediately, then fire `refresh_snapshots` in the background for local repos. Remote repos skip the background refresh by default to avoid unnecessary network calls; enabling `remote_auto_refresh` in Settings makes them behave like local repos.
- After `run_backup` succeeds: parse the new `snapshot_id` from the restic NDJSON summary line, fetch that single snapshot's metadata (`restic snapshots --json <id>`), and prepend it to the cached list — no full re-fetch needed.
- After `forget_by_plan` succeeds: run `restic snapshots --json` to repopulate the snapshot cache with the post-prune list.
- Stats cache (`get_repo_stats` / `refresh_repo_stats`): for remote repos, stats are evicted after backup/forget rather than auto-repopulated, since `restic stats` reads pack indexes which can be large on remote storage.
- `clear_browse_cache` command wipes all three cache tables (exposed to frontend for manual cache clearing).

## Adding a New Feature

1. Add a `#[tauri::command]` function in the appropriate `src-tauri/src/commands/*.rs` file.
   - If the command needs to call restic, accept `State<'_, AppDb>` and `State<'_, MasterKey>`, call `master_key.get()?` to obtain the key, then `db.get_full_repo(&repo_id, &key)?` to retrieve decrypted credentials.
2. Register it in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.
3. Add a typed wrapper in `src/lib/invoke.ts`.
4. Consume it from a page or hook.

## Theming

The app supports three modes — Dark (default), Light, and System (follows OS) — stored in `localStorage` via `src/lib/theme.tsx` and applied as a class (`dark`, `light`, `system`) on `<html>`.

### How colors are themed

All theme-sensitive Tailwind utilities route through CSS custom properties defined in `src/index.css`. Tailwind's `gray`, `blue`, and `green` color scales are extended in `tailwind.config.js` to reference these variables:

```
gray.50–950   → --tw-gray-50 … --tw-gray-950
blue.300/400/700/900 → --tw-blue-300/400/700/900
green.400     → --tw-green-400
```

`:root` holds the dark-mode defaults (standard Tailwind gray scale, original blue/green values). `html.light` and `@media (prefers-color-scheme: light) html.system` override them with the light-mode palette.

**Light mode palette**: Tailwind `slate` color family (cool blue-gray tint) reversed — `gray-950` → slate-50 background, `gray-100` → slate-900 near-black text. Blue accents remap to darker values (blue-400/300 → blue-700) so they remain legible on light backgrounds. Green-400 remaps to green-700.

### Adding or changing a themed color

1. Add `--tw-<color>-<shade>: <R> <G> <B>;` to both `:root` (dark default) and `html.light` (light override) in `src/index.css`.
2. Extend `tailwind.config.js` under `theme.extend.colors` with `"rgb(var(--tw-<color>-<shade>) / <alpha-value>)"`.
3. Use `text-<color>-<shade>` / `bg-<color>-<shade>` in components as usual — the value adapts per theme automatically.

### Hardcoded colors to avoid

- Do **not** use `text-white` for text on gray backgrounds — it becomes invisible in light mode. Use `text-gray-50` instead (remaps to near-black in light, near-white in dark).
- Do **not** use `hover:text-white` on interactive elements inside content areas — same reason. Use `hover:text-gray-50`.
- Colors outside the extended set (e.g. `blue-500`, `blue-600`, `red-*`, `yellow-*`) are **not** theme-mapped and render identically in both modes. This is intentional for colored-background elements like primary buttons (`bg-blue-600 text-white`) where white text is always on a dark-colored surface.

## Releases

`.github/workflows/release.yml` builds and publishes releases. Triggered by pushing a `v*` tag. Runs three parallel jobs (ubuntu-22.04, macos-latest, windows-latest) using `tauri-apps/tauri-action@v0`, which creates a draft GitHub Release and uploads platform artifacts. The annotated tag message becomes the release body. Uses `Swatinem/rust-cache` for Rust dependency caching and `actions/setup-node` (Node 24) npm caching to speed up repeat builds. The build job is gated with `if: github.server_url == 'https://github.com'` so it is silently skipped on Gitea or other CI systems. The job requires `permissions: contents: write` so `GITHUB_TOKEN` can create releases — no manual token setup needed.

Pre-built macOS binaries are not code-signed or notarized. Users must remove the quarantine attribute after installing: `sudo xattr -rd com.apple.quarantine /Applications/Resty\ Desktop.app`. This is documented in the README. The `augment_path()` fix ensures restic and rclone resolve correctly from a downloaded build even when the app is launched from Finder (which provides a minimal PATH).

To cut a release, use the `/tag` slash command to generate the annotated tag message, then push:

```bash
# e.g.: /tag v0.0.5,v0.0.6
git push origin main      # workflow file must be on main before the tag is pushed
git push origin v0.0.6
```

## Running the App

Rust must be installed first (it is not bundled):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# restart terminal, then:
npm install
npm run tauri dev
```

Build a distributable:

```bash
npm run tauri build
```

Remove build artifacts (`dist/` and `src-tauri/target/`):

```bash
npm run clean
```
