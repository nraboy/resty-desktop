# Resty Desktop

A cross-platform desktop client for the Restic CLI backup tool.

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
| Single-instance | `tauri-plugin-single-instance` — prevents multiple processes; focuses existing window on relaunch |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Restic integration | `std::process::Command` with `--json` flag |

## Project Structure

```
src/
  App.tsx          # Router + layout shell; auth state machine (loading/setup/locked/unlocked);
                   #   ErrorBoundary catches render errors; restic version warning banner on unlock
  main.tsx         # React entry; suppresses context menu globally
  index.css        # Tailwind directives + global styles
  components/
    Button.tsx     # primary/secondary/danger/ghost variants
    ContextMenu.tsx # Portal-rendered right-click menu; auto-nudges onto screen; closes on Escape/click-outside
    EmptyState.tsx # Empty list placeholder
    Input.tsx      # Labeled input with error state; optional onClear prop shows inline × when value non-empty;
                   #   className applies to outer wrapper div (not <input>); <input> is always w-full inside wrapper
    Modal.tsx      # Overlay modal dialog
    Sidebar.tsx    # Left nav with app icon + repo indicator
  lib/
    types.ts       # Shared TS types: Repository, Snapshot, FileEntry, ResticStats, SnapshotStats, CheckResult,
                   #   BackupHistoryEntry, BackupProgress, RestoreProgress, RetentionPolicy, BackupPlan,
                   #   DiffEntry, DiffResult; isRemoteRepo() helper
    invoke.ts      # Typed wrappers over tauri invoke()
    format.ts      # formatBytes, formatSize, formatDate, formatTimestamp, formatDuration
    config.ts      # MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR constants for version warning
    theme.tsx      # ThemeProvider + useTheme(); persists to localStorage; applies dark/light/system class to <html>
  pages/
    AuthPage.tsx            # Master password setup (first launch) and unlock screen
    RepositoriesPage.tsx    # Add/open/delete repos; restic init for new repos; remote URL support;
                            #   per-row and bulk stats refresh; mirror, edit, check, prune via right-click context menu;
                            #   edit modal: name/path/password with Test Connection; prune: confirmation→progress→done
    SnapshotsPage.tsx       # Snapshot table; stale-while-revalidate cache; inline tag editor; delete with prune option;
                            #   full-snapshot restore with streaming progress; per-snapshot copy with cancellation;
                            #   pagination (PAGE_SIZE=10); filter with × clear; right-click context menu;
                            #   multi-select mode: bulk delete and copy with progress bars
    BrowsePage.tsx          # File tree inside a snapshot; per-entry and multi-select restore; breadcrumb nav;
                            #   restore modal with strip_leading_path option; inline tag management
    DiffPage.tsx            # Diff viewer at /snapshots/:repoId/diff/:snapshotA/:snapshotB;
                            #   client-side tree from flat entries; summary bar; restore from diff; truncation warning
    BackupPlansPage.tsx     # List/run/delete plans; backup modal with streaming progress + cancellation;
                            #   auto-applies retention after successful backup; per-plan Apply Retention button
    BackupPlanEditPage.tsx  # Create/edit plan (name, repo, paths, tags, excludes, retention, bandwidth limits);
                            #   exclude patterns: Simple tab (tag list + presets) / Expert tab (freeform textarea)
    SchedulesPage.tsx       # List schedules; toggle/delete/run; amber warning when tray disabled
    ScheduleEditPage.tsx    # Create/edit schedule (name, cron expr, backup plans); scheduleId="new" for creation
    LogsPage.tsx            # Backup history log; paginated (PAGE_SIZE=10); expandable error rows
    SettingsPage.tsx        # Theme selector; tray + remote-auto-refresh toggles; restic binary path;
                            #   compression selector; default restore path; prune all repos with streaming progress

src-tauri/
  src/
    main.rs        # Calls restic_gui_lib::run()
    lib.rs         # Tauri builder; registers all commands; manages AppDb, MasterKey, CopyHandle, MirrorHandle,
                   #   BackupHandle, PruneHandle as state; native menu bar (auth-aware, skipped on Linux);
                   #   system tray created lazily after unlock (activate_tray); TRAY_GEN counter avoids ID collisions;
                   #   window close → hide-to-tray if tray_enabled, else exit; RunEvent::Reopen (macOS only)
    commands/
      mod.rs         # get_restic_path(); NoConsole trait: no_console() + augment_path() for Finder-launched PATH
      auth.rs        # is_app_setup, setup_master_password, unlock_app (clears stale locks), lock_app,
                     #   change_master_password, reset_app
      crypto.rs      # Argon2id key derivation, AES-GCM encrypt/decrypt
      repo.rs        # list/add/remove/init/rename/update repos; get_repo_password; test_repo_connection;
                     #   get/refresh_repo_stats; get/set_restic_path; get_restic_version; check_repo;
                     #   get/set_compression; get/set_restore_path; get/set_tray_enabled;
                     #   get/set_remote_auto_refresh; prune_all_repos; prune_repo; cancel_prune
      snapshot.rs    # list/refresh/delete/tag snapshots; get_snapshot_stats; execute_backup (shared pub async fn);
                     #   run_backup; cancel_backup; apply_retention (shared pub fn); forget_by_plan;
                     #   copy_snapshot; cancel_copy; mirror_repo; cancel_mirror; unlock_repo; diff_snapshots;
                     #   validate_snapshot_id() (pub(crate), 8–64 hex) guards all snapshot ID inputs here and in browse.rs
      browse.rs      # list_files; restore_path (strip_leading_path moves restored item to target root);
                     #   restore_snapshot (streaming restore:progress events); EA-error suppression on Windows;
                     #   all three validate snapshot_id via snapshot::validate_snapshot_id
      backup_plan.rs # list/save/remove backup plans; sorted alphabetically by name
      schedule.rs    # list/save/remove/toggle schedules; run_schedule_now; describe_cron_expr
      cache.rs       # AppDb (SQLite state); MasterKey; CopyHandle; MirrorHandle; BackupHandle (with busy flag); PruneHandle;
                     #   rotate_master_key (atomic key rotation); recalculate_overdue_schedules;
                     #   list_backup_history + log_backup trim, both bounded by BACKUP_HISTORY_LIMIT (1000, newest-first)
  scheduler.rs       # 60s background tick; runs due schedules via execute_backup; applies retention after backup;
                     #   skips when locked or when a backup is already running (busy flag); AtomicBool guards against overlapping ticks
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
- All commands set `RESTIC_REPOSITORY` and `RESTIC_PASSWORD` env vars — never pass either in process args.
- `restic ls --json` outputs NDJSON; first line is snapshot summary (skipped); subsequent lines are `FileEntry`.
- `execute_backup` streams NDJSON line-by-line; `status` lines → `backup:progress` events; `summary` line captured and returned. Fires notification on completion. Reads compression from `app_settings` (`RESTIC_COMPRESSION` env). Accepts `limit_upload`/`limit_download` (KiB/s); `Some(0)` treated as `None`. Serialized via a `busy` flag on `BackupHandle` — only one backup runs at a time; a concurrent attempt (e.g. a scheduler tick firing during a manual backup) returns `"A backup is already in progress"`. Sequential callers (`run_schedule_now`, scheduler loop) are unaffected since each `await` releases the flag before the next starts.
- `cancel_backup`, `cancel_copy`, `cancel_mirror`, `cancel_prune` all run `restic unlock` after SIGKILL to clear stale locks.
- `copy_snapshot` runs `restic copy --from-repo <src> <snapshot_id>` against the destination repo.
- `mirror_repo` uses `RESTIC_FROM_REPOSITORY`/`RESTIC_FROM_PASSWORD` env vars to copy all snapshots src→dest.
- `diff_snapshots` parses plain-text `restic diff` output (no `--json`); prefixes `+`/`-`/`M`/`T`; capped at 500 entries with `truncated` flag. DiffPage always navigates older→newer so `+` = added in newer.
- `check_repo` runs `restic check --json`; duration measured via `Instant` (no timing in summary). Returns `CheckResult { success, errors, duration_seconds }`.
- `restore_snapshot` streams `restic restore --json`; emits `restore:progress` events. Stderr drained on background thread.
- `unlock_app` runs `restic unlock` on all repos in background after password verified.
- Stats cache evicted after backup/forget for remote repos; not auto-repopulated (restic stats reads full pack indexes).

## Security Architecture

- Master password → Argon2id → 32-byte key; AES-GCM encrypts verification plaintext; salt+nonce+ciphertext stored in `master_key` table. Password never stored.
- All repo passwords AES-GCM encrypted with master key in `repositories` table; decrypted on-demand via `db.get_full_repo`.
- `MasterKey` is `Mutex<Option<[u8; 32]>>` as Tauri state; `None` when locked — all restic commands fail with "App is locked".
- `change_master_password` calls `db.rotate_master_key`, which re-encrypts all repo passwords **and** rewrites the `master_key` verification row in a single SQLite transaction (all-or-nothing — a crash can't leave passwords on the new key while the verification blob still expects the old one). The intermediate decrypted password is zeroized per row.
- `reset_app` wipes all SQLite tables and clears in-memory key.

## Persistence & Caching

- Single SQLite `app_data.db` in Tauri app data dir. Tables: `master_key`, `repositories`, `backup_plans`, `schedules`, `app_settings`, `snapshots_cache`, `browse_cache`, `repo_stats_cache`, `backup_history`.
- `list_snapshots` returns from cache only; `refresh_snapshots` calls restic and updates cache.
- SnapshotsPage: stale-while-revalidate — serve cache immediately, background refresh for local repos.
- After `run_backup`: new snapshot metadata prepended to cache (no full re-fetch).
- After `forget_by_plan`: full `restic snapshots --json` repopulates cache.
- `clear_browse_cache` wipes all three cache tables.
- `backup_history` is bounded: `log_backup` trims to the newest `BACKUP_HISTORY_LIMIT` (1000) rows after each insert, matching the read limit so the Logs page never loses visible rows.

## Adding a New Feature

1. Add `#[tauri::command]` in the appropriate `src-tauri/src/commands/*.rs` file. For restic calls: accept `State<'_, AppDb>` + `State<'_, MasterKey>`, call `master_key.get()?`, then `db.get_full_repo(&repo_id, &key)?`.
2. Register in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.
3. Add a typed wrapper in `src/lib/invoke.ts`.
4. Consume from a page.

## Theming

Three modes: Dark (default), Light, System. Stored in `localStorage`; applied as `dark`/`light`/`system` class on `<html>`.

All theme-sensitive colors route through CSS custom properties in `src/index.css`. Extended in `tailwind.config.js`:
```
gray.50–950, blue.300/400/700/900, green.400
```
`:root` = dark defaults. `html.light` and `@media (prefers-color-scheme: light) html.system` override with light palette (slate family, reversed).

### Adding a themed color
1. Add `--tw-<color>-<shade>: <R> <G> <B>;` to `:root` and `html.light` in `src/index.css`.
2. Extend `tailwind.config.js` under `theme.extend.colors`.
3. Use `text-<color>-<shade>` / `bg-<color>-<shade>` as usual.

### Hardcoded colors to avoid
- `text-white` on gray backgrounds → use `text-gray-50` (remaps to near-black in light mode).
- `hover:text-white` on interactive elements → use `hover:text-gray-50`.
- `bg-red-700` for buttons → theme-mapped, becomes pastel pink in light mode. Use `bg-red-600 hover:bg-red-800`.
- Colors outside the extended set (`blue-500/600`, `red-500/6/8`, `yellow-*`) are NOT theme-mapped — intentional for colored-background elements like primary/danger buttons where white text is always on a dark surface.

## Releases

`.github/workflows/release.yml` — triggered by `v*` tag; builds on ubuntu-22.04, macos-latest, windows-latest via `tauri-apps/tauri-action@v0`; creates a draft GitHub Release. Annotated tag message becomes release body. Requires `permissions: contents: write`. Skipped on non-GitHub CI (`github.server_url` check).

Pre-built macOS binaries are not notarized: `sudo xattr -rd com.apple.quarantine /Applications/Resty\ Desktop.app`.

To cut a release, use `/tag` then:
```bash
git push origin main
git push origin v0.0.X
```

## Running the App

```bash
npm install
npm run tauri dev   # requires Rust installed
npm run tauri build # distributable
npm run clean       # remove dist/ and src-tauri/target/
```
