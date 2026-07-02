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
    ImportExportCard.tsx # Settings card: export all repos/plans/schedules to an encrypted
                   #   .json file, and import (preview→confirm) as fresh copies; import modal
                   #   tabs between Resty Export and Backrest config.json
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
    utils.ts       # needsFullDiskAccess(paths): returns true if any path matches macOS protected prefixes (~/Library, /System, /private, /var)
    theme.tsx      # ThemeProvider + useTheme(); persists to localStorage; applies dark/light/system class to <html>
  pages/
    AuthPage.tsx            # Master password setup (first launch) and unlock screen
    RepositoriesPage.tsx    # Add/open/delete repos; restic init for new repos; remote URL support;
                            #   per-row and bulk stats refresh; mirror, edit, check, prune via right-click context menu;
                            #   edit modal: name/path/password with Test Connection; prune: confirmation→progress→done
    SnapshotsPage.tsx       # Snapshot table; stale-while-revalidate cache; inline tag editor; delete with prune option;
                            #   full-snapshot restore with streaming progress; per-snapshot copy with cancellation;
                            #   pagination (PAGE_SIZE=10); filter with × clear; right-click context menu;
                            #   multi-select mode: bulk delete and copy with progress bars;
                            #   per-row "Index Snapshot" / "Remove Index" context-menu item toggles based on index status:
                            #   shows "Index Snapshot" (disabled while in_progress) or "Remove Index" (active when complete);
                            #   "Remove Index" calls clear_snapshot_index and removes the snapshot from the local status map;
                            #   "Index Snapshot" shows a progress modal; listens for index:done to update per-row status map live;
                            #   listens for snapshots:refreshed to reload list when warmer updates cache;
                            #   per-row and context-menu "Search Files" button → SearchPage
    BrowsePage.tsx          # File tree inside a snapshot; per-entry and multi-select restore; breadcrumb nav;
                            #   restore modal with strip_leading_path option; inline tag management;
                            #   "Search" button navigates to SearchPage, passing returnPath+returnStack so back
                            #   navigation can restore the current directory depth; accepts initialPath+initialPathStack
                            #   from SearchPage so "open in browser" lands at the right directory;
                            #   fromSearch flag in location state changes back-button destination (navigate(-1)
                            #   restores search state from history entry written by window.history.replaceState)
    SearchPage.tsx          # Full-text file search within a single snapshot at /snapshots/:repoId/:snapshotId/search;
                            #   requires snapshot to be indexed (browse_cache_files); shows index state machine
                            #   (loading→not_indexed→indexing→ready); "Index Now" triggers index_snapshot;
                            #   listens for index:done to transition to ready; debounced 300ms search via
                            #   search_snapshot_files (SQLite LIKE, capped at 200 results); clicking a result
                            #   writes restoredQuery+restoredResults into current history entry via
                            #   window.history.replaceState before navigating to BrowsePage (so navigate(-1)
                            #   restores them); back button (fromBrowse) navigates explicitly to BrowsePage
                            #   with returnPath+returnStack to restore the correct directory depth;
                            #   searchSeqRef guards against out-of-order responses — a burst of keystrokes can
                            #   have several (slow, ~1s+) searches in flight, so only the response matching the
                            #   latest call is applied to state
    RepoSearchPage.tsx      # File search across every indexed snapshot in a repo at /snapshots/:repoId/search;
                            #   same index/debounce/stale-response-guard pattern as SearchPage.tsx, but backed by
                            #   search_repo_files, which dedups each matching path to the newest snapshot
                            #   containing it (shown as a snapshot short-id badge per result; clicking opens that
                            #   snapshot's BrowsePage). Banner shows "Searching N of M snapshots" with an "Index
                            #   All" action when the repo is only partially indexed; a modal with a real
                            #   progress bar (derived from index:done events matched against the batch's target
                            #   snapshot ids) tracks the Index All run
    DiffPage.tsx            # Diff viewer at /snapshots/:repoId/diff/:snapshotA/:snapshotB;
                            #   client-side tree from flat entries; summary bar; restore from diff; truncation warning
    BackupPlansPage.tsx     # List/run/delete plans; backup modal with streaming progress + cancellation;
                            #   auto-applies retention after successful backup; per-plan Apply Retention button;
                            #   pre-flight FDA check before running: warns if plan includes protected paths and FDA not granted (macOS only)
    BackupPlanEditPage.tsx  # Create/edit plan (name, repo, paths, tags, excludes, retention, bandwidth limits);
                            #   exclude patterns: Simple tab (tag list + presets) / Expert tab (freeform textarea);
                            #   amber FDA warning suppressed when FDA is confirmed granted (macOS only)
    SchedulesPage.tsx       # List schedules; toggle/delete/run; amber warning when tray disabled
    ScheduleEditPage.tsx    # Create/edit schedule (name, cron expr, backup plans); scheduleId="new" for creation
    LogsPage.tsx            # Backup history log; paginated (PAGE_SIZE=10); expandable error rows
    SettingsPage.tsx        # Theme selector; tray + auto-indexing + remote-auto-refresh toggles; restic binary path;
                            #   compression selector; default restore path; prune all repos with streaming progress;
                            #   import/export card (ImportExportCard);
                            #   cache management: "Clean Orphaned" (remove stale rows) + "Clear All Cache" (wipe + VACUUM);
                            #   DB size display (app_data.db + WAL) refreshes after each cache operation;
                            #   Full Disk Access card (macOS only): green when granted, amber with instructions + Re-check when not

src-tauri/
  src/
    main.rs        # Calls restic_gui_lib::run()
    lib.rs         # Tauri builder; registers all commands; manages AppDb, MasterKey, CopyHandle, MirrorHandle,
                   #   BackupHandle, PruneHandle as state; native menu bar (auth-aware, skipped on Linux) with
                   #   Import/Export and Help items; system tray created lazily after unlock (activate_tray);
                   #   TRAY_GEN counter avoids ID collisions; window close → hide-to-tray if tray_enabled, else exit;
                   #   RunEvent::Reopen (macOS only)
    commands/
      mod.rs         # get_restic_path(); NoConsole trait: no_console() + augment_path() for Finder-launched PATH
      auth.rs        # is_app_setup, setup_master_password, unlock_app (clears stale locks), lock_app,
                     #   change_master_password, reset_app
      crypto.rs      # Argon2id key derivation, AES-GCM encrypt/decrypt
      repo.rs        # list/add/remove/init/rename/update repos; get_repo_password; test_repo_connection;
                     #   get/refresh_repo_stats; get/set_restic_path; get_restic_version; check_repo;
                     #   get/set_compression; get/set_restore_path; get/set_tray_enabled;
                     #   get/set_remote_auto_refresh; get/set_auto_indexing (default false);
                     #   prune_all_repos; prune_repo; cancel_prune;
                     #   check_full_disk_access (macOS only — probes TCC.db; returns {supported, granted});
                     #   open_full_disk_access_settings (macOS only — deep-links to Privacy & Security pane)
      snapshot.rs    # list/refresh/delete/tag snapshots; get_snapshot_stats; execute_backup (shared pub async fn);
                     #   run_backup; cancel_backup; apply_retention (shared pub fn); forget_by_plan;
                     #   copy_snapshot; cancel_copy; mirror_repo; cancel_mirror; unlock_repo; diff_snapshots;
                     #   validate_snapshot_id() (pub(crate), 8–64 hex) guards all snapshot ID inputs here and in browse.rs
      browse.rs      # list_files; restore_path (strip_leading_path moves restored item to target root);
                     #   restore_snapshot (streaming restore:progress events); EA-error suppression on Windows;
                     #   all three validate snapshot_id via snapshot::validate_snapshot_id;
                     #   index_snapshot (fire-and-forget manual indexing, emits index:done when complete);
                     #   get_snapshot_index_status (map of snapshot_id → "pending"|"in_progress"|"complete");
                     #   clear_snapshot_index: deletes browse_cache_files + browse_cache_status for one snapshot via db.evict();
                     #   run_full_index (pub(crate) shared with cache_warmer): runs restic ls --json and bulk-inserts into browse_cache_files;
                     #   search_snapshot_files (requires "complete" index): LIKE search on name+path in browse_cache_files, capped at 200 results;
                     #   search_repo_files: LIKE search across every "complete" snapshot in a repo via AppDb::search_repo_files,
                     #   capped at 200 results; both search commands are async and run the actual query via
                     #   tauri::async_runtime::spawn_blocking — see Persistence & Caching for why
      backup_plan.rs # list/save/remove backup plans; sorted alphabetically by name
      schedule.rs    # list/save/remove/toggle schedules; run_schedule_now; describe_cron_expr;
                     #   next_fire_time() (pub(crate)) reused by scheduler.rs and transfer.rs
      transfer.rs    # export_data/preview_import/import_data; portable .json bundle (readable,
                     #   only repo passwords encrypted under an export passphrase); every object has its
                     #   own id, refs by id; import mints fresh UUIDs + remaps refs, " (imported)" name dedup;
                     #   preview_backrest_import/import_backrest_config: one-way import of Backrest config.json
                     #   (plaintext pw re-encrypted under master key; lossy — see Import / Export)
      cache.rs       # AppDb (SQLite state); MasterKey; CopyHandle; MirrorHandle; BackupHandle (with busy flag); PruneHandle;
                     #   rotate_master_key (atomic key rotation); recalculate_overdue_schedules;
                     #   list_backup_history + log_backup trim, both bounded by BACKUP_HISTORY_LIMIT (1000, newest-first);
                     #   clear_cache: DELETE all cache tables + PRAGMA wal_checkpoint(TRUNCATE) + VACUUM to reclaim disk space;
                     #   get_db_size: sums app_data.db + app_data.db-wal for accurate WAL-mode reporting;
                     #   search_browse_files: SQLite LIKE search on browse_cache_files (name OR path), escapes metacharacters, limit param;
                     #   search_repo_files: repo-wide variant — joins browse_cache_files/snapshots_cache/browse_cache_status,
                     #   GROUP BY path + MAX(time) dedups each matching path down to the newest snapshot containing it
  cache_warmer.rs    # Background sweep spawned at unlock; 10s initial delay, then 60s tick forever.
                     #   Each tick: (1) refresh_all_snapshots — always runs, calls restic snapshots --json for every
                     #   eligible repo and updates snapshots_cache, emits snapshots:refreshed per repo;
                     #   (2) trigger_sweep — only runs if auto_indexing=true, continuously indexes one uncached
                     #   snapshot at a time via run_full_index until nothing remains, emits index:done per snapshot.
                     #   Both phases respect remote_auto_refresh (skip remote repos when disabled).
                     #   AtomicBool running prevents overlapping file-index sweeps.
  scheduler.rs       # 60s background tick; runs due schedules via execute_backup; applies retention after backup;
                     #   skips when locked or when a backup is already running (busy flag); AtomicBool guards against overlapping ticks
```

## Routes

| Path | Page |
|---|---|
| `/` | RepositoriesPage |
| `/snapshots/:repoId` | SnapshotsPage |
| `/snapshots/:repoId/search` | RepoSearchPage |
| `/snapshots/:repoId/:snapshotId/browse` | BrowsePage |
| `/snapshots/:repoId/:snapshotId/search` | SearchPage |
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

- Single SQLite `app_data.db` in Tauri app data dir. Tables: `master_key`, `repositories`, `backup_plans`, `schedules`, `app_settings`, `snapshots_cache`, `indexed_snapshots`, `browse_cache_files`, `browse_cache_status`, `repo_stats_cache`, `backup_history`.
- Browse cache is relational (v0→v1 migration): `browse_cache_files` stores the file tree keyed by `(snap, parent_path)`; `browse_cache_status` tracks index state per `(repo_id, snapshot_id)` as `pending`/`in_progress`/`complete`, plus a per-snapshot `cached_at`. Replaces the old JSON-blob `browse_cache`.
- v1→v2 migration (storage optimization): `browse_cache_files.snapshot_id` (64-char hex, duplicated across the row and both its indexes) is interned to a small integer `snap` via a new `indexed_snapshots(id, snapshot_id UNIQUE)` table — `AppDb::intern_snapshot`/`snap_id_of` map hex↔int internally; all public `AppDb` methods still take the hex `snapshot_id`. The redundant `name` column (recomputed from `path` via `cache::name_of` on read) and the per-row `cached_at` (moved to `browse_cache_status`, one value per snapshot) were also dropped. Cache tables are disposable (rebuilt via `restic ls`), so this migration drops + recreates them rather than transforming data.
- `list_snapshots` returns from cache only; `refresh_snapshots` calls restic and updates cache.
- SnapshotsPage: stale-while-revalidate — serve cache immediately, background refresh for local repos.
- After `run_backup`: new snapshot metadata prepended to cache (no full re-fetch).
- After `forget_by_plan`: full `restic snapshots --json` repopulates cache.
- `remove_repo` cascades to `browse_cache_status`, `browse_cache_files`, `snapshots_cache`, and `repo_stats_cache`.
- `clear_browse_cache` (Clear All Cache): DELETEs all cache tables, then `PRAGMA wal_checkpoint(TRUNCATE)` + `VACUUM` to reclaim disk space from the WAL and compact the main file.
- `backup_history` is bounded: `log_backup` trims to the newest `BACKUP_HISTORY_LIMIT` (1000) rows after each insert, matching the read limit so the Logs page never loses visible rows.
- Background cache warmer: every 60s, snapshot metadata is refreshed for all eligible repos (always). File indexing (browse_cache) is pre-populated in the background only when `auto_indexing` is enabled. Both phases skip remote repos unless `remote_auto_refresh` is on.
- `AppDb` holds one `Mutex<Connection>` — every command using `AppDb` shares that single lock, so a slow synchronous query (e.g. a repo-wide `LIKE` search over hundreds of thousands of cached file rows, ~1s+) held on a core async-runtime thread starves *every other* command that also touches `AppDb` (snapshot refreshes, index-status polling, the cache warmer tick) until it finishes. Any new command doing DB work slow enough to notice should be `async fn` and run the actual query via `tauri::async_runtime::spawn_blocking` (see `search_snapshot_files`/`search_repo_files`, or the existing `index_snapshot`/`restore_snapshot`) so it occupies a blocking-pool thread instead of a scarce core worker thread.

## Import / Export

- `transfer.rs` exports a portable `.json` bundle (`version: 1` schema; also records `appVersion` from `tauri.conf.json` for debugging — informational only, ignored on import). Only repo passwords are encrypted: decrypted with the master key, re-encrypted with an Argon2id key derived from a user-supplied **export passphrase** (fresh 16-byte salt stored in the bundle; nonce+ciphertext base64). Passphrase required only when the bundle includes repositories.
- App settings, backup history, and caches are excluded. Every object carries its own `id`; plans reference `repoId` and schedules reference `planIds` by id, so the file is self-describing and safe to hand-edit.
- Export is always a **full snapshot** — every repo, plan, and schedule, verbatim (no selection UI). The export modal is just a passphrase prompt (shown only when repos exist).
- Import always creates **fresh copies**: new UUIDs minted Rust-side, refs remapped, names de-duplicated with a `" (imported)"` suffix; schedule timing reset (`next_run_at` recomputed via `schedule::next_fire_time`, `created_at = now`). Imported schedules are **always disabled** (`enabled = false`) regardless of their source state, so backups don't fire before the user reviews paths on the new host. All inserts run in one transaction via `AppDb::import_bundle` (all-or-nothing). Paths are imported verbatim — the import preview warns they may not exist on the new machine.
- Dangling references are tolerated, never fatal: a plan whose repo isn't in the file (orphaned by a repo deletion) imports with `repo_id = ""` (reassign in the editor); schedule refs to absent plans are dropped. So a plan with no valid repo still round-trips with its config intact.
- `preview_import` returns counts without a passphrase (only secrets are encrypted); it verifies the passphrase early only if one is supplied.

### Backrest import (one-way)

- `preview_backrest_import`/`import_backrest_config` import a Backrest (`github.com/garethgeorge/backrest`) `config.json` as fresh copies (same fresh-UUID + `" (imported)"` dedup + `import_bundle` transaction path). No export passphrase: Backrest stores repo passwords in plaintext (`password` field, or `RESTIC_PASSWORD=` in `env`), which are re-encrypted under the local master key.
- Mapping: repo `uri`→`path`, repo `id`→`name`; plan `repo`→`repoId`, `iexcludes` folded into `excludes`; retention oneof (`policyKeepLastN`/`policyTimeBucketed`)→`RetentionPolicy`; Backrest's per-plan embedded `schedule.cron` becomes one Resty `Schedule` per plan (disabled/`maxFrequency*` schedules dropped).
- **Lossy by design** — silently dropped: hooks, restic `flags`/`env`, `commandPrefix`, repo `prunePolicy`/`checkPolicy`, `skipIfUnchanged`/`autoUnlock`/`autoInitialize`, `clock`, multihost/auth, hourly retention, plan tags (Backrest auto-tags), bandwidth limits. The import preview shows a generic "not everything will carry over" warning. All Backrest structs use `#[serde(default)]` so partial/older configs still parse.

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

## Testing

- Frontend tests use **Vitest**; test files live alongside source as `src/lib/*.test.ts`.
- Rust unit tests use `#[cfg(test)]` modules in `commands/cache.rs`, `commands/crypto.rs`, `commands/snapshot.rs`, and `commands/transfer.rs`.
- CI (`.github/workflows/test.yml`) runs on every push that isn't a `v*` tag and on PRs.

```bash
npm run test:vite   # frontend tests only
npm run test:rust   # Rust tests only (cargo test)
npm run test:all    # both
```

## Running the App

```bash
npm install
npm run tauri dev   # requires Rust installed
npm run tauri build # distributable
npm run clean       # remove dist/ and src-tauri/target/
```
