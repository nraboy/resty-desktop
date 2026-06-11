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
| Styling | Tailwind CSS v3 |
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
  App.tsx                     # Router + layout shell; handles auth state machine (loading/setup/locked/unlocked)
  main.tsx                    # React entry point
  index.css                   # Tailwind directives + global styles
  components/
    Button.tsx                # Reusable button (primary/secondary/danger/ghost variants)
    EmptyState.tsx            # Empty list placeholder
    Input.tsx                 # Labeled input with error state
    Modal.tsx                 # Overlay modal dialog
    Sidebar.tsx               # Left nav with app icon + "Resty Desktop" title; active repo indicator
  lib/
    types.ts                  # Shared TS types: Repository, Snapshot, FileEntry, ResticStats, CheckResult,
                              #   BackupHistoryEntry, BackupProgress, RestoreProgress, RetentionPolicy, BackupPlan; isRemoteRepo() helper
    invoke.ts                 # Typed wrappers over tauri invoke()
    format.ts                 # Shared display formatters: formatBytes, formatSize, formatDate, formatTimestamp,
                              #   formatDuration (fractional param for sub-minute precision); used by all pages
  pages/
    AuthPage.tsx              # Master password setup (first launch) and unlock screen; shown before main UI
    RepositoriesPage.tsx      # Add/open/delete repos; triggers restic init for new repos; supports remote URLs (S3, SFTP, etc.);
                              #   mirror repository modal: copies all snapshots from one repo to another with indeterminate progress bar,
                              #   elapsed timer, cancellation support, and completion/cancellation confirmation UI
    SnapshotsPage.tsx         # Table of snapshots; inline tag editor; delete with prune option; stale-while-revalidate cache pattern; on-demand repo check;
                              #   full-snapshot restore modal with streaming progress bar (restore:progress events);
                              #   per-snapshot copy to another repo with cancellation support
    BrowsePage.tsx            # File tree navigation inside a snapshot; per-entry restore; breadcrumb nav;
                              #   inline tag management (add/remove tags on the snapshot directly from the browse view)
    BackupPlansPage.tsx       # List saved backup plans; run a plan immediately; delete plans;
                              #   backup modal with streaming progress bar (backup:progress events), cancellation support
                              #   (cancel_backup), and completion/error confirmation UI
    BackupPlanEditPage.tsx    # Create/edit a backup plan (name, repo, paths, tags, excludes, retention policy); planId="new" for creation;
                              #   exclude patterns use tabbed Simple (tag list) / Expert (freeform textarea) UI
    SchedulesPage.tsx         # List scheduled backups; toggle enabled/disabled; delete; run immediately
    ScheduleEditPage.tsx      # Create/edit a schedule (name, cron expression, backup plans to run); scheduleId="new" for creation
    LogsPage.tsx              # Persistent backup history log; shows date, plan, repo, duration, file counts, bytes added, snapshot ID; expandable error rows
    SettingsPage.tsx          # Restic binary path override; shows detected restic version below path input;
                              #   install instructions section hidden when restic is found;
                              #   global backup compression selector (off/fastest/auto/better/max) persisted to app_settings

src-tauri/
  Cargo.toml
  tauri.conf.json
  src/
    main.rs                   # Calls restic_gui_lib::run()
    lib.rs                    # Tauri builder; registers all commands; opens app_data.db, initialises schema, manages AppDb + MasterKey as Tauri state;
                              #   manages CopyHandle, MirrorHandle, BackupHandle as Tauri state;
                              #   calls recalculate_overdue_schedules at startup to skip missed schedule runs;
                              #   builds native menu bar (MenuState) with "Resty Desktop" and "File" submenus; items are auth-aware
                              #   (Settings/New Repository/New Backup Plan shown when unlocked; Reset Application shown when locked);
                              #   menu events emitted as menu:new-repository, menu:new-backup-plan, menu:settings, menu:reset-app Tauri events;
                              #   set_menu_auth_state command called from frontend after auth transitions
    commands/
      mod.rs                  # shared get_restic_path() helper used by all command modules
      auth.rs                 # is_app_setup, setup_master_password, unlock_app, lock_app,
                              #   change_master_password, reset_app;
                              #   unlock_app runs restic unlock on all repos in the background to clear stale locks from crashes
      crypto.rs               # Argon2id key derivation, AES-GCM encrypt/decrypt helpers
      repo.rs                 # list_repos, add_repo, remove_repo, init_repo, rename_repo,
                              #   test_repo_connection, get_repo_stats, refresh_repo_stats, get/set_restic_path,
                              #   get_restic_version, check_repo, get_compression, set_compression
      snapshot.rs             # list_snapshots, refresh_snapshots, delete_snapshot, tag_snapshot,
                              #   execute_backup (pub async helper shared by run_backup, run_schedule_now, scheduler.rs),
                              #   run_backup (delegates to execute_backup), cancel_backup (kills BackupHandle child),
                              #   forget_by_plan, unlock_repo,
                              #   copy_snapshot (streams to dest repo with cancellation), cancel_copy;
                              #   mirror_repo (copies all snapshots src→dest via restic copy, cancellable), cancel_mirror;
                              #   both cancel_copy, cancel_mirror, and cancel_backup run restic unlock after SIGKILL to clear stale locks
      browse.rs               # list_files, restore_path, restore_snapshot
      backup_plan.rs          # list_backup_plans, save_backup_plan, remove_backup_plan; plans stored in SQLite
      schedule.rs             # list_schedules, save_schedule, remove_schedule, toggle_schedule,
                              #   run_schedule_now (accepts BackupHandle, calls execute_backup for each plan in the schedule),
                              #   describe_cron_expr; cron helpers next_fire_time + describe_cron
      cache.rs                # AppDb (unified SQLite state); MasterKey (in-memory, zeroizes key bytes on set/clear);
                              #   FullRepository derives ZeroizeOnDrop so password is wiped from memory when dropped;
                              #   CopyHandle (in-memory, for cancel);
                              #   MirrorHandle (in-memory, for mirror cancellation);
                              #   BackupHandle (in-memory, same Arc<Mutex<Child>> + AtomicBool pattern, for backup cancellation);
                              #   AppDb::recalculate_overdue_schedules — advances overdue next_run_at to next future fire time at startup (skips missed backups);
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
- `run_backup` delegates to `execute_backup` (a shared `pub async fn` also used by `run_schedule_now` and `scheduler.rs`). `execute_backup` streams NDJSON from restic stdout line-by-line; `status` lines are parsed and emitted as `backup:progress` Tauri events (consumed by the frontend progress bar); the final `summary` line is captured and returned (all other lines are discarded as they arrive — not buffered). On completion, fires a system notification (success or failure) via `tauri-plugin-notification` and writes a row to `backup_history`. A `BackupHandle` state (same `Arc<Mutex<Option<Child>>>` + `AtomicBool` pattern as `CopyHandle`) allows `cancel_backup` to kill the child process mid-run; after cancel-kill, `restic unlock` is called on the repo to clear stale locks. `execute_backup` reads the `compression` setting from `app_settings` (default `auto`) and passes it as `RESTIC_COMPRESSION` env var to the restic process.
- `check_repo` runs `restic check --json`; progress/status lines go to stderr (ignored), only the summary lands on stdout. Duration is measured via `std::time::Instant` since the summary message contains no timing field. Returns `CheckResult { success, errors, duration_seconds }`.
- `restore_snapshot` streams `restic restore <id> --target <dir> --json` stdout line-by-line; `status` lines are parsed and emitted as `restore:progress` Tauri events (consumed by the frontend progress bar in the restore modal). Stderr is drained on a background thread and surfaced as the error message on non-zero exit.
- `copy_snapshot` runs `restic copy --from-repo <src> <snapshot_id>` against the destination repo; streams stdout and surfaces errors via stderr. A `CopyHandle` Tauri state (Arc<Mutex<Option<Child>>> + AtomicBool cancelled) allows `cancel_copy` to kill the child process mid-run. After a cancel-kill, `restic unlock` is called on both repos to clear stale locks.
- `mirror_repo` runs `restic copy` (no specific snapshot ID) with `RESTIC_FROM_REPOSITORY`/`RESTIC_FROM_PASSWORD` env vars to copy all snapshots from src to dest, skipping those already present. A `MirrorHandle` state (same pattern as `CopyHandle`) allows `cancel_mirror`. Also runs `restic unlock` on both repos after cancel.
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
- `SnapshotsPage` uses a stale-while-revalidate pattern: serve cache immediately, then fire `refresh_snapshots` in the background for local repos. Remote repos skip the background refresh to avoid unnecessary network calls.
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
