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
| Memory safety | `zeroize` crate — `MasterKey` and `FullRepository` zeroize sensitive bytes on drop/replace; `FullRepository` also derives `Clone` (each clone still zeroizes independently) so one-shot restic calls can own their repo across a `spawn_blocking` boundary |
| Notifications | `tauri-plugin-notification` — shown on backup success/failure |
| Single-instance | `tauri-plugin-single-instance` — prevents multiple processes; focuses existing window on relaunch |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Restic integration | `std::process::Command` with `--json` flag; one-shot calls run via `run_restic_blocking` (`repo.rs`), which runs on a `spawn_blocking` thread so they never occupy an async-runtime worker |

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
    ActivityPanel.tsx # Right-side slide-in drawer (slim always-visible rail + fixed overlay) surfacing
                   #   background activity with no other visibility: auto-indexing progress, scheduler-
                   #   triggered backups (Active Tasks — driven by the unified `task` event bus filtered
                   #   to origin "scheduler", see Operation Event Bus and lib/activity.tsx's
                   #   reduceSchedulerBackup; Stop wired to cancelBackup(), which kills whatever's
                   #   in BackupHandle.child regardless of manual/scheduler origin; shown only during the
                   #   "backup" phase, hidden during "retention" since apply_retention has no cancel path —
                   #   subtitle swaps to "Applying retention rules…" so the ~10-20s forget isn't mistaken for
                   #   a frozen bar), in-flight repo stats refreshes (also in Active Tasks — lifecycle-only,
                   #   no progress bar), next few due schedules (Upcoming Tasks — rows truncate with
                   #   a hover tooltip for long plan lists), and last few backup runs (Recent Logs — neutral
                   #   "Cancelled" glyph instead of red-X/"Failed" for CANCELLED_BACKUP_ERROR entries).
                   #   Restore/copy/mirror/manual backup/prune have their own progress modals and are
                   #   intentionally excluded — see lib/activity.tsx.
  lib/
    types.ts       # Shared TS types: Repository, Snapshot, FileEntry, ResticStats, SnapshotStats, CheckResult,
                   #   BackupHistoryEntry, BackupProgress, RestoreProgress, RetentionPolicy, BackupPlan,
                   #   DiffEntry, DiffResult; isRemoteRepo() helper; CANCELLED_BACKUP_ERROR sentinel (see
                   #   snapshot.rs's execute_backup) distinguishing a genuine cancel from a real failure
    invoke.ts      # Typed wrappers over tauri invoke()
    activity.tsx   # ActivityProvider (mounted once in App.tsx, outlives route changes since it must keep
                   #   updating no matter which page is mounted): indexing progress, the scheduler-triggered
                   #   activeBackup (never a manual/"Run Now" backup — derived from the unified `task` bus
                   #   filtered to origin "scheduler" via the pure, unit-tested reduceSchedulerBackup
                   #   reducer, replacing the legacy scheduler:backup-started/backup:progress/
                   #   scheduler:retention-started/scheduler:backup-finished events outright — see Operation
                   #   Event Bus) carrying a phase ("backup"|"retention") flipped by the retention step's own
                   #   `forget`-kind task op reaching "started"; a plan with no retention configured never
                   #   gets a `forget` op, so that case is instead dismissed by a plan-lookup effect once it
                   #   confirms no keep_* flag is set (see reduceSchedulerBackup's doc comment for why the
                   #   reducer alone can't know this), upcoming due schedules (refreshed on schedules:changed,
                   #   which the scheduler emits after record_schedule_run advances next_run_at — NOT on the
                   #   task bus, which fires per-plan before the advance and would read a
                   #   stale past timestamp), recentLogs, and statsRefreshing/statsFailed — repoId sets
                   #   derived (via the pure, unit-tested reduceStatsOps reducer, StatsOpsState) from the
                   #   unified `task` event bus filtered to kind "stats" rather than from a per-operation
                   #   feed (stats never had one). Lifecycle-only, no error text: the reducer tracks
                   #   operationId→repoId across started (also clears any prior failure marker for that
                   #   repo)/finished/failed/cancelled to drive a spinner (statsRefreshing) and a plain
                   #   boolean "last attempt failed" marker (statsFailed, no message — see repo.rs's
                   #   fetch_and_cache_stats, where every failure path reports through task_ctx.failed(...)
                   #   explicitly so this marker never depends on the invoke promise's own rejection). The
                   #   actual numbers are re-read from the DB cache by RepositoriesPage's own `task`
                   #   listener (only on "finished"), not carried on the event. Powers ActivityPanel.tsx and
                   #   (for statsRefreshing/statsFailed) RepositoriesPage.tsx directly.
    format.ts      # formatBytes, formatSize, formatDate, formatDateOnly, formatTimestamp, formatDuration
    config.ts      # MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR constants for version warning
    utils.ts       # needsFullDiskAccess(paths): returns true if any path matches macOS protected prefixes (~/Library, /System, /private, /var)
    theme.tsx      # ThemeProvider + useTheme(); persists to localStorage; applies dark/light/system class to <html>
  pages/
    AuthPage.tsx            # Master password setup (first launch) and unlock screen
    RepositoriesPage.tsx    # Add/open/delete repos; restic init for new repos; remote URL support;
                            #   per-row and bulk stats refresh (manual-only — no auto-eviction; see Restic
                            #   Integration; "Refresh All" always includes remote repos, unlike every
                            #   automatic remote activity); spinner (statsRefreshing) and failure marker
                            #   (statsFailed, a plain boolean — no error text, see activity.tsx) both come
                            #   from ActivityProvider's `task`-bus subscription and survive navigating away
                            #   mid-refresh; row data comes from a page-local `task` listener re-reading
                            #   get_repo_stats on "finished" (a guaranteed cache hit); each row shows a
                            #   "Refreshed …" label from cached_at, and a failed refresh keeps the last-good
                            #   value visible with a plain "refresh failed" marker rather than blanking to
                            #   "unavailable"; mirror, edit, check, prune, "Index All Snapshots" via
                            #   right-click context menu; edit modal: name/path/password with Test
                            #   Connection; prune: confirmation→progress→done; "Index All Snapshots"
                            #   opens the same dismissible progress/queued/Stop/complete modal pattern as
                            #   RepoSearchPage's own "Index All" (independent state, its own `task`
                            #   listener scoped to whichever repo the context menu targeted — deliberate
                            #   duplication, see "Known, deferred frontend duplication" below), and calls
                            #   the same index_snapshots_batch/getActiveIndexBatch/cancel_index_batch
                            #   commands, so a batch started from either page is visible in both
                            #   (and in ActivityPanel) and adopted rather than duplicated; the menu item is
                            #   disabled per-repo via a page-local `repoNeedsIndexing` map (cache-only
                            #   listSnapshots+get_snapshot_index_status reads, recomputed on repo-list
                            #   changes and kept live via the `task` bus + snapshots:refreshed — fails
                            #   open/enabled while unchecked)
    SnapshotsPage.tsx       # Snapshot table; stale-while-revalidate cache; inline tag editor; delete with prune option;
                            #   full-snapshot restore with streaming progress; per-snapshot copy with cancellation;
                            #   pagination (PAGE_SIZE=10); filter with × clear; right-click context menu;
                            #   multi-select mode: bulk delete and copy with progress bars;
                            #   per-row "Index Snapshot" / "Remove Index" context-menu item toggles based on index status:
                            #   shows "Index Snapshot" (disabled while in_progress) or "Remove Index" (active when complete);
                            #   "Remove Index" calls clear_snapshot_index and removes the snapshot from the local status map;
                            #   "Index Snapshot" shows a progress modal; listens for `task` events (kind "index") to
                            #   update per-row status map live; listens for snapshots:refreshed to reload list when
                            #   warmer updates cache;
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
                            #   listens for `task` events (kind "index") to transition to ready; debounced 300ms search via
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
                            #   All" action when the repo is only partially indexed; "Index All" calls
                            #   index_snapshots_batch once (backend indexes sequentially, one snapshot at a time,
                            #   pausing the auto-indexer for the run — see browse.rs); a modal with a real
                            #   progress bar (derived from `task` events, kind "index", matched against the batch's
                            #   target snapshot ids via targetId) tracks the run, with a Stop button (cancel_index_batch; takes effect
                            #   between snapshots) shown while in progress; the batch also survives the modal being
                            #   dismissed — see ActivityPanel.tsx
    DiffPage.tsx            # Diff viewer at /snapshots/:repoId/diff/:snapshotA/:snapshotB;
                            #   client-side tree from flat entries; summary bar; restore from diff; truncation warning
    BackupPlansPage.tsx     # List/run/delete plans; backup modal with streaming progress + cancellation
                            #   (cancelling shows a local "Stopping…" state, then reverts to the Start Backup
                            #   view — no distinct "cancelled" UI block, matching cancel_backup's own behavior);
                            #   auto-applies retention after successful backup; per-plan Apply Retention button;
                            #   pre-flight FDA check before running: warns if plan includes protected paths and FDA not granted (macOS only)
    BackupPlanEditPage.tsx  # Create/edit plan (name, repo, paths, tags, excludes, retention, bandwidth limits);
                            #   exclude patterns: Simple tab (tag list + presets) / Expert tab (freeform textarea);
                            #   amber FDA warning suppressed when FDA is confirmed granted (macOS only)
    SchedulesPage.tsx       # List schedules; toggle/delete/run; amber warning when tray disabled
    ScheduleEditPage.tsx    # Create/edit schedule (name, cron expr, backup plans); scheduleId="new" for creation
    LogsPage.tsx            # Backup history log; paginated (PAGE_SIZE=10); expandable error rows (only for a
                            #   real failure — a CANCELLED_BACKUP_ERROR entry renders a neutral "Cancelled"
                            #   glyph instead of the red error icon, and isn't expandable)
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
                   #   BackupHandle, PruneHandle, RestoreHandle, IndexHandle, RepoLocks as state; native menu bar (auth-aware, skipped on Linux) with
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
                     #   open_full_disk_access_settings (macOS only — deep-links to Privacy & Security pane);
                     #   run_restic_blocking() (pub(crate) async helper): runs a one-shot restic command on a
                     #   spawn_blocking thread, owning its args so they can cross that boundary — used by every
                     #   async command that shells out once (see Restic Integration for the full policy);
                     #   check_repo/refresh_repo_stats each acquire a RepoLocks read guard (see Concurrency
                     #   section — get_repo_stats is a plain sync, cache-only DB read with no restic call and
                     #   so no lock of its own; see its doc comment in repo.rs for why a cache miss returns Err
                     #   rather than falling through to a live fetch); prune_repo/prune_all_repos share a run_one_prune_attempt helper
                     #   (spawns the child, polls via try_wait, captures stderr, retries twice on restic's own
                     #   "already locked"); they take a RepoLocks write guard first and re-check
                     #   PruneHandle::cancelled right after acquiring it (closes a Stop-during-the-lock-wait
                     #   orphan-process gap), and run_one_prune_attempt's cancelled branch makes its own kill
                     #   attempt on the child before clearing PruneHandle::child (closes a second, narrower race
                     #   where a concurrent cancel_prune saw `None` because it ran before the child was stored —
                     #   Child::drop doesn't kill). Both prune commands also carry a `busy` guard on PruneHandle
                     #   (a second concurrent attempt fails fast with "already in progress" instead of corrupting
                     #   the shared child/cancelled state)
      repo_locks.rs  # RepoLocks: in-memory per-repo-path shared/exclusive lock registry (see Concurrency section)
      snapshot.rs    # list/refresh/delete/tag snapshots; get_snapshot_stats; execute_backup (shared pub async fn);
                     #   run_backup; cancel_backup; apply_retention (shared pub fn, intentionally sync — see
                     #   Intentional Designs); forget_by_plan (async, runs apply_retention via spawn_blocking,
                     #   takes an optional plan_id, calls log_retention_failure on error); copy_snapshot;
                     #   cancel_copy; mirror_repo; cancel_mirror; unlock_repo; diff_snapshots;
                     #   validate_snapshot_id() (pub(crate), 8–64 hex) guards all snapshot ID inputs here and in
                     #   browse.rs; list_snapshots returns Vec<Snapshot> directly from AppDb::get_snapshots_vec
                     #   (no JSON round-trip); CANCELLED_BACKUP_ERROR sentinel ("Cancelled") — execute_backup's
                     #   Err branch logs/notifies a genuine cancellation distinctly instead of the raw internal
                     #   "cancelled" string, which would otherwise always read as "Backup failed";
                     #   log_retention_failure (pub(crate)) records a failed retention as its own backup_history
                     #   row ("Retention failed: <err>") so it's visible in Recent Logs/LogsPage even though
                     #   apply_retention has no history entry of its own — called from all three retention call
                     #   sites (forget_by_plan, the scheduler tick, run_schedule_now); copy_snapshot/mirror_repo
                     #   each carry a `busy` guard on CopyHandle/MirrorHandle (a second concurrent attempt fails
                     #   fast with "already in progress" instead of corrupting shared state); execute_backup/
                     #   copy_snapshot/mirror_repo/refresh_snapshots/get_snapshot_stats/diff_snapshots each
                     #   acquire a RepoLocks read guard, delete_snapshot/tag_snapshot/apply_retention each
                     #   acquire a write guard (see Concurrency section)
      browse.rs      # list_files; restore_path (strip_leading_path moves restored item to target root);
                     #   restore_snapshot (streaming restore:progress events); EA-error suppression on Windows;
                     #   all three validate snapshot_id via snapshot::validate_snapshot_id;
                     #   index_snapshot (fire-and-forget manual indexing; reports completion solely via a `task`
                     #   event, kind "index" — no legacy per-operation event, see Operation Event Bus);
                     #   index_snapshots_batch ("Index All": fire-and-forget, indexes snapshot_ids sequentially
                     #   one at a time in a single spawned task — bounds memory to one snapshot's file list;
                     #   emits one `task` event per snapshot, same targetId/repoId shape as index_snapshot, *plus*
                     #   a batch-level `task` op (no targetId — that's the discriminator from the per-snapshot
                     #   events) that reports `progress` with itemsDone/itemsTotal as the batch advances, with its
                     #   own fresh cancel flag + task slot registered in IndexHandle::batches under its own
                     #   operationId (not shared across batches — lets concurrent "Index All" runs, e.g. for
                     #   different repos, proceed and cancel fully independently) so cancel_index_batch(operation_id)
                     #   targets exactly one batch, and is what lets ActivityPanel show each batch's progress as
                     #   its own row, independent of RepoSearchPage's own modal — see activity.tsx's
                     #   reduceIndexBatches; a failed snapshot doesn't abort the batch, and the loop checks its own
                     #   cancel flag between snapshots, never mid-restic); cancel_index_batch (looks up the (cancel,
                     #   task slot) pair for the given operation_id in IndexHandle::batches and, if found, sets
                     #   cancel — a no-op if that batch already finished); both index_snapshot and index_snapshots_batch set
                     #   IndexHandle::manual_active for their duration (cleared via a ManualIndexGuard Drop impl
                     #   so it stays set for the whole run) so cache_warmer's auto-indexer pauses during manual
                     #   indexing, and take IndexHandle::gate (tokio::sync::Mutex<()>, held across run_full_index's
                     #   spawn_blocking) so a manual index can never overlap an in-flight auto-indexed snapshot;
                     #   get_snapshot_index_status (map of snapshot_id → "pending"|"in_progress"|"complete");
                     #   clear_snapshot_index: deletes browse_cache_files + browse_cache_status for one snapshot
                     #   via db.evict(); run_full_index (pub(crate), shared with cache_warmer): runs restic ls
                     #   --json and bulk-inserts into browse_cache_files; list_files/restore_path/
                     #   restore_snapshot/run_full_index each acquire a RepoLocks read guard, held across the
                     #   restic call for the whole child-process lifetime (see Concurrency section);
                     #   search_snapshot_files (requires "complete" index): LIKE search on name+path in
                     #   browse_cache_files, capped at 200; search_repo_files: LIKE search across every
                     #   "complete" snapshot in a repo via AppDb::search_repo_files, capped at 200; both search
                     #   commands are async and run the query via tauri::async_runtime::spawn_blocking — see
                     #   Persistence & Caching for why
      backup_plan.rs # list/save/remove backup plans; sorted alphabetically by name
      schedule.rs    # list/save/remove/toggle schedules; run_schedule_now (calls log_retention_failure on a
                     #   retention error, same as forget_by_plan/scheduler.rs); describe_cron_expr;
                     #   next_fire_time() (pub(crate)) reused by scheduler.rs and transfer.rs
      transfer.rs    # export_data/preview_import/import_data; portable .json bundle (readable,
                     #   only repo passwords encrypted under an export passphrase); every object has its
                     #   own id, refs by id; import mints fresh UUIDs + remaps refs, " (imported)" name dedup;
                     #   preview_backrest_import/import_backrest_config: one-way import of Backrest config.json
                     #   (plaintext pw re-encrypted under master key; lossy — see Import / Export)
      cache.rs       # AppDb (SQLite state); MasterKey; CopyHandle; MirrorHandle; BackupHandle (with busy flag); PruneHandle
                     #   (CopyHandle/MirrorHandle/PruneHandle each also have a `busy` guard, closing a gap where they
                     #   previously shared their handle with no serialization — a concurrent second run could clobber
                     #   the first run's child/cancelled state; a second concurrent attempt now returns a clean
                     #   "already in progress" error, matching BackupHandle/RestoreHandle's existing pattern);
                     #   rotate_master_key (atomic key rotation); recalculate_overdue_schedules;
                     #   get_snapshots_vec: reads snapshots_cache rows straight into Vec<Snapshot> (paths/tags JSON
                     #   columns parsed once) — no intermediate JSON-string serialization;
                     #   list_backup_history + log_backup trim, both bounded by BACKUP_HISTORY_LIMIT (1000, newest-first);
                     #   idx_history_started index on backup_history(started_at); log_backup's trim DELETE is
                     #   guarded by a COUNT(*) check so a normal insert (table under the cap) skips it;
                     #   remove_repo's cascade deletes run in one transaction (all-or-nothing);
                     #   clear_cache: DELETE all cache tables + PRAGMA wal_checkpoint(TRUNCATE) + VACUUM to reclaim disk space;
                     #   get_db_size: sums app_data.db + app_data.db-wal for accurate WAL-mode reporting;
                     #   search_browse_files: SQLite LIKE search on browse_cache_files (name OR path), escapes metacharacters, limit param;
                     #   search_repo_files: repo-wide variant — joins browse_cache_files/snapshots_cache/browse_cache_status,
                     #   GROUP BY path + MAX(time) dedups each matching path down to the newest snapshot containing it
  cache_warmer.rs    # Background sweep spawned at unlock; 10s initial delay, then 60s tick forever.
                     #   Each tick: (1) refresh_all_snapshots — always runs, calls restic snapshots --json for every
                     #   eligible repo and updates snapshots_cache, emits snapshots:refreshed per repo;
                     #   (2) trigger_sweep — only runs if auto_indexing=true, continuously indexes one uncached
                     #   snapshot at a time via run_full_index until nothing remains, reporting each snapshot
                     #   solely via a `task` event (kind "index", origin "background").
                     #   Both phases respect remote_auto_refresh (skip remote repos when disabled).
                     #   AtomicBool running prevents overlapping file-index sweeps. trigger_sweep/index_next also
                     #   check IndexHandle::manual_active and yield (sweep stops cleanly, retries next tick) while
                     #   manual indexing (index_snapshot / index_snapshots_batch, browse.rs) is active; index_next's
                     #   run_full_index call takes IndexHandle::gate to close the race against an in-flight manual
                     #   index — see browse.rs's index_snapshots_batch doc comment;
                     #   refresh_all_snapshots and index_next's run_full_index call each acquire a RepoLocks read
                     #   guard (see Concurrency section)
  scheduler.rs       # Background tick sleeps until the next wall-clock minute boundary (:00) via
                     #   secs_until_next_minute (unit-tested), not a flat 60s from last-tick — keeps tick times
                     #   predictable/aligned instead of drifting with however long each tick took. Runs due
                     #   schedules via execute_backup; applies retention after backup, calling
                     #   log_retention_failure (snapshot.rs) if it errors so the failure isn't silently swallowed;
                     #   skips when locked or when a backup is already running (busy flag); AtomicBool guards
                     #   against overlapping ticks. Per-plan Activity-panel visibility rides entirely on the
                     #   `task` bus now (see Operation Event Bus) — no events are emitted from this file
                     #   itself for it: execute_backup's own OperationCtx (kind Backup, origin Scheduler)
                     #   covers the backup phase, and apply_retention's (kind Forget, same origin, only
                     #   created when retention actually runs — a plan with no keep_* flag set never gets
                     #   one) covers retention. The three legacy scheduler:backup-started/
                     #   retention-started/backup-finished events this file used to emit alongside were
                     #   retired outright once activity.tsx's reduceSchedulerBackup migrated onto the bus —
                     #   the frontend now infers "plan fully done" from the retention op's own terminal
                     #   event (or, for a no-retention plan, from a plan lookup confirming none is coming)
                     #   rather than a dedicated finished signal from this loop. Before the first plan starts:
                     #   record_schedule_run advances next_run_at, then emits schedules:changed so Upcoming Tasks
                     #   refreshes to the next fire time immediately — not after all plans + retention finish, which
                     #   would otherwise leave Upcoming Tasks showing the stale past due-time for the entire run
                     #   (activity.tsx refreshes upcoming on schedules:changed, NOT on backup-finished). Note: the
                     #   minute-boundary tick still bounds how soon a newly-due schedule starts (up to ~60s after
                     #   its due instant) — this is tick granularity, not RepoLocks contention (backup and indexing
                     #   both take non-blocking read guards).
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
- Every restic `Command` sets `.stdin(Stdio::null())` alongside `.no_console()` (`mod.rs`'s `NoConsole` trait). Without it, stdin defaults to `Stdio::inherit()`, which in a windowless Tauri app on Windows can be an invalid handle; process wrappers that do their own internal `CreateProcess` (e.g. Scoop shims) can fail to spawn the real binary when handed that invalid inherited handle ("Could not create process" errors, unrelated to restic/SSH/SFTP config). Any new restic `Command` must set `.stdin(Stdio::null())` too.
- All restic subprocess calls run off the async runtime: streaming commands spawn a `Child` inside `tauri::async_runtime::spawn_blocking` (e.g. `execute_backup`, `restore_snapshot`, `copy_snapshot`, `mirror_repo`); one-shot commands go through `run_restic_blocking` (`repo.rs`), which does the same for a single `Command::output()` call. `prune_all_repos`/`prune_repo` are the one exception to the "spawn inside `spawn_blocking`" pattern: they spawn their `Child` inline with null stdio and poll it via `try_wait` + `tokio::time::sleep` in short lock windows, so `cancel_prune` is never blocked waiting on the mutex — this is intentional, not a gap. Their cancel-path `restic unlock` (clearing the lock after a kill) goes through `run_restic_blocking` like every other one-shot call. An `async fn` `#[tauri::command]` must never call `std::process::Command` (or `run_restic_with_path`) inline — that blocks a shared tokio worker and starves every other async command and the `AppDb` lock (see Persistence & Caching). Plain `run_restic_with_path` is still called directly from code that's already inside a `spawn_blocking` closure (e.g. `run_full_index`) — that's correct as-is, not a gap.
- `restic ls --json` outputs NDJSON; first line is snapshot summary (skipped); subsequent lines are `FileEntry`.
- `execute_backup` streams NDJSON line-by-line; `status` lines → `backup:progress` events; `summary` line captured and returned. Fires notification on completion. Reads compression from `app_settings` (`RESTIC_COMPRESSION` env). Accepts `limit_upload`/`limit_download` (KiB/s); `Some(0)` treated as `None`. Serialized via a `busy` flag on `BackupHandle` — only one backup runs at a time; a concurrent attempt (e.g. a scheduler tick firing during a manual backup) returns `"A backup is already in progress"`. Sequential callers (`run_schedule_now`, scheduler loop) are unaffected since each `await` releases the flag before the next starts. On a genuine cancellation (`BackupHandle::cancelled` set), the `Err` branch logs `CANCELLED_BACKUP_ERROR` ("Cancelled") to `backup_history` instead of the raw `"cancelled"` control-flow string, and fires a "Backup cancelled" notification instead of "Backup failed" — see `CANCELLED_BACKUP_ERROR` in `lib/types.ts` and `LogsPage`/`ActivityPanel`'s matching neutral rendering.
- `cancel_backup`, `cancel_copy`, `cancel_mirror`, `cancel_prune`, `cancel_restore` all run `restic unlock` after SIGKILL to clear stale locks.
- `copy_snapshot` runs `restic copy --from-repo <src> <snapshot_id>` against the destination repo.
- `mirror_repo` uses `RESTIC_FROM_REPOSITORY`/`RESTIC_FROM_PASSWORD` env vars to copy all snapshots src→dest.
- `diff_snapshots` parses plain-text `restic diff` output (no `--json`); prefixes `+`/`-`/`M`/`T`; capped at 500 entries with `truncated` flag. DiffPage always navigates older→newer so `+` = added in newer.
- `check_repo` runs `restic check --json`; duration measured via `Instant` (no timing in summary). Returns `CheckResult { success, errors, duration_seconds }`.
- `restore_snapshot` streams `restic restore --json`; emits `restore:progress` events. Stderr drained on background thread. Serialized via a `busy` flag on `RestoreHandle` (same pattern as `BackupHandle`) — a concurrent attempt returns `"A restore is already in progress"`. Cancellable via `cancel_restore`; on cancel, a successful exit still wins over the cancelled flag (handles the race where Stop is clicked right as the restore finishes).
- `unlock_app` runs `restic unlock` on all repos in background after password verified.
- Stats cache (`repo_stats_cache`) is **never auto-evicted** — not after backup, forget/retention, copy, mirror, or snapshot delete. It only changes via the Refresh row/Refresh All buttons on RepositoriesPage (`refresh_repo_stats`), which is a deliberate, user-driven-only model (the previous event-driven eviction made the page feel like it "refreshed at random" — same page, different states, for reasons the user never triggered). A failed refresh leaves the last-good cached value in place. Each row shows the cached value's `cached_at` as a "Refreshed …" label (see `ResticStats.cached_at`, `repo_stats_cache.cached_at`).

## Concurrency: Per-Repository Lock Registry

Restic distinguishes **shared** locks (most commands — `backup`, `restore`, `copy`, `mirror`'s
`copy`, `check`, `snapshots`, `stats`, `ls`) from **exclusive** locks (`forget`, `prune`, `tag` —
nothing else may touch the repo while one runs). The app had no cross-operation awareness of
this, so an exclusive op could fire mid-shared-op and fail with restic's own "repository is
already locked by PID …" — reproduced by starting a manual backup, clicking Refresh on
Repositories (`refresh_repo_stats`) while it runs, and watching the backup's post-run retention
collide with that still-in-flight `stats` call.

`RepoLocks` (`src-tauri/src/commands/repo_locks.rs`, managed state) is an in-memory
`HashMap<repo_path, {readers, exclusive}>` — keyed by **repository path** (restic's true lock
identity, so two `repo_id`s pointing at the same path correctly serialize), not `repo_id`. Two
RAII guards, both releasing on `Drop`:

- **`ReadGuard`** (`RepoLocks::read`) — shared-lock ops acquire this. **Never blocks**; just
  increments a counter and returns immediately. Readers are deliberately one-directional — they
  never wait for writers — so a slow exclusive op can't make a listing/stats call hang.
- **`WriteGuard`** (`RepoLocks::write` async / `write_blocking` sync) — exclusive-lock ops
  acquire this. Polls until the repo has zero readers and isn't already exclusive, then
  atomically claims it — **waits genuinely, no timeout or force-claim.** An earlier version
  force-claimed after 15s, which reintroduced the exact collision this registry exists to
  prevent whenever the shared op it waited on ran longer than 15s — a confirmed regression, not
  hypothetical. Restic's own lock, plus the retry below, remain the backstop for a genuine
  residual collision (e.g. an external restic/cron process `RepoLocks` can't see).

Wired into every shared-lock op (`execute_backup`, `restore_snapshot`, `restore_path`,
`copy_snapshot`/`mirror_repo` — both src **and** dest — `refresh_snapshots`,
`get_snapshot_stats`, `diff_snapshots`, `refresh_repo_stats`, `check_repo`,
`list_files`, `run_full_index` — shared by `index_snapshot`/`index_snapshots_batch` **and** the
`cache_warmer` auto-sweep) and every exclusive-lock op (`delete_snapshot`, `tag_snapshot`,
`prune_repo`/`prune_all_repos`, `apply_retention` — covering all three callers: `forget_by_plan`,
the scheduler tick, `run_schedule_now`). For a streaming op the guard is a local held across the
`spawn_blocking(...).await`, claimed for the whole child-process lifetime. `restic unlock` calls
(cancel paths, `unlock_app`) are **exempt** — they're the recovery mechanism and must never wait.

`prune_repo`/`prune_all_repos` re-check `PruneHandle::cancelled` right after acquiring their
write guard and before spawning the child — `write()`'s wait has no cancellation hook, so
without this a Stop click during that wait would leave an unkillable orphaned `restic prune`
running while the app reported "Cancelled". A second, narrower race exists at the moment the
child is stored: a concurrent `cancel_prune` between `spawn()` returning and the child landing in
`PruneHandle::child` would see `None` and no-op. `run_one_prune_attempt`'s polling loop makes its
own kill attempt the moment it observes `cancelled`, closing the gap regardless of which side of
the race fired (killing an already-exited child is a harmless no-op).

`RepoLocks` only coordinates this app's own operations — it can't see a different machine or tool
(restic CLI, Backrest, another Resty Desktop instance) genuinely holding the repo's real restic
lock. All four exclusive-lock commands (`delete_snapshot`, `tag_snapshot` via
`run_restic_blocking_retrying_on_lock`; `prune_repo`/`prune_all_repos` via
`run_one_prune_attempt`) retry up to twice, 2s apart, on restic's own "already locked" error
before surfacing it, matching `apply_retention`'s original retry pattern. `prune_repo`/
`prune_all_repos` capture stderr for this (previously discarded via `Stdio::null()`), so a prune
failure surfaces restic's actual error text instead of a generic "Prune failed".

Coverage is intentionally partial-safe: since writers only wait on the `readers` counter (never
the reverse), an un-instrumented reader just degrades to pre-`RepoLocks` behavior for that
pairing — it can't introduce a new failure, only leave one collision un-prevented. Don't
"complete" this by making readers wait for writers too — a slow exclusive op on a large/remote
repo would then make snapshot listings and stats hang, a worse regression than the rare
collision this registry exists to prevent.

## Operation Event Bus

`src-tauri/src/tasks.rs` defines a second, **uniform** event layer on top of the ad-hoc
per-operation events described above (`backup:progress`, `restore:progress`,
`scheduler:*`). Those events grew one at a time, so their payloads are
inconsistent — some carry no id at all, some only a display name, and roughly half the restic
operations (`copy`, `mirror`, single-repo `prune`, `forget`/retention, `check`, `diff`,
`restore_path`, `unlock`) emit nothing. The `task` event bus exists so every operation reports a
consistent, correlatable lifecycle — **in addition to**, never instead of, its existing detailed
feed — so a future background-task consumer has a uniform contract to build on instead of
retrofitting every operation at that point.

**Two layers, deliberately kept separate:**
- **`task` (this bus)** — one Tauri event, uniform envelope, every covered operation. This is the
  coordination layer a future subscriber uses.
- **Existing detailed feeds** (`backup:progress`, etc.) — unchanged, still power every shipped
  modal and the Activity panel. Rich, per-kind detail (current file names, ETA) lives here, not in
  the normalized `task` envelope.

**Envelope** (`TaskEvent`, camelCase over the wire):
`operationId` (unique per operation instance — see below), `kind` (`backup`|`restore`|
`restorePath`|`copy`|`mirror`|`prune`|`forget`|`tag`|`check`|`diff`|`index`|`unlock`|`stats`|
`testConnection`|`browse`|`init`), `phase`
(`started`|`progress`|`cancelling`|`cancelled`|`finished`|`failed`), `repoId`, `targetId`
(plan/snapshot/schedule id, when one applies), `origin` (`manual`|`scheduler`|`background`),
`progress` (normalized `percentDone`/`itemsDone`/`itemsTotal`/`bytesDone`/`bytesTotal`/`label`,
plus `secondsElapsed`/`secondsRemaining`/`currentFiles`/`repoId` — per-kind detail kept lossless
vs the legacy `backup:progress`/`restore:progress` payloads even though no consumer reads it yet
(`currentFiles`/`secondsRemaining` are backup-only, `repoId` is prune-all's per-tick repo,
distinct from the envelope's own `repoId` which is `""` for a multi-repo prune) — only on
`phase: progress`), `error` (only on `phase: failed`), `at` (unix millis).

**Why `operationId` is the core of the design, not an afterthought:** today's per-operation events
get away with carrying no id (or just a display name) only because of this app's single-in-flight
`busy` flags — one backup, one restore, one prune at a time. A future background-task system that
runs operations concurrently breaks that invariant, at which point `repoId` alone can no longer
tell two simultaneous operations apart. `operationId` (a 16-char alphanumeric id, same scheme as
`backup_history.id`) is generated once per operation and threaded through every event for its
lifetime specifically so that retrofit never has to happen.

**`OperationCtx<S: TaskSink>`** owns one operation's lifecycle: `OperationCtx::new(...)` emits
`started`; `.progress_emitter()` returns a cheap `Clone`-able `TaskProgressEmitter` for emitting
`progress` from inside a `spawn_blocking` closure (the ctx itself stays in the outer async scope
so it can read the final `Result`); exactly one of `.finished()` / `.failed(err)` / `.cancelled()`
must be called on every exit path. If none is called (an unhandled early return or panic unwind),
`Drop` emits a trailing `failed("operation dropped")` — a **backstop only**, not the intended
path; every wired call site is expected to call a terminal method explicitly (see the
`'body: { ... break 'body Err(...) }` labeled-block pattern in `prune_all_repos`/`prune_repo`,
used specifically so every one of their several early-return points still reports through
`OperationCtx` instead of falling through to the Drop backstop). `TaskSink` is a trait (implemented
for `AppHandle`) purely so `tasks.rs`'s tests can record emitted events without a real app.

Cancellable operations (backup, restore, copy, mirror, prune) carry a `current_task: TaskSlot`
(`Arc<Mutex<Option<TaskRef>>>`) on their existing handle (`BackupHandle`, `RestoreHandle`, ...) —
`OperationCtx::new` publishes its `TaskRef` (including the operation's `origin`, so
`emit_cancelling` reports the operation's real origin rather than assuming every cancel is
user-initiated) there on start and clears it on terminal; the matching `cancel_*` command calls
`emit_cancelling(&app, &handle.current_task)` right before its existing kill/stop logic, so
`cancelling` always precedes the `cancelled`/`finished` the operation itself emits once it actually
stops. Operations with no cancel path (check, diff, tag, unlock, forget, single-snapshot
`index_snapshot`) pass `None` for the slot.

The `index_snapshots_batch` ("Index All") batch is a deliberate exception to the shared-handle
`current_task` pattern above: since multiple batches (e.g. for different repos) can run
concurrently, a single `TaskSlot` on `IndexHandle` would let a second batch silently steal the
first's cancel target (a real bug this design replaced — see `IndexHandle::batches`' doc comment
in `cache.rs`). Instead, each batch creates its **own** fresh cancel flag + `TaskSlot` and
registers the pair in `IndexHandle::batches: Arc<Mutex<HashMap<operationId, BatchCancel>>>` for
its duration (deregistered on any exit via `BatchDeregisterGuard`, mirroring `ManualIndexGuard`'s
Drop pattern). `cancel_index_batch(operation_id)` looks up that specific batch's entry and calls
`emit_cancelling`/sets its cancel flag — a no-op if the batch already finished. `cancelling` only
means "no further snapshots will start" — the snapshot already in flight still finishes normally
(`finished`/`failed`), since cancellation is checked only between snapshots, never mid-`restic`.

**Coverage:** every restic-shelling operation is wired, including the click-bounded metadata reads
(`refresh_repo_stats` — via the shared `fetch_and_cache_stats` helper, `not` the outer command —
`get_snapshot_stats`, `test_repo_connection`, `list_files`). Two categories are excluded, deliberately:
- **Not real restic operations at all** — `list_snapshots` (`AppDb::get_snapshots_vec`, pure cached
  DB read), `get_snapshot_index_status` (sync DB read), and `get_repo_stats` (sync, cache-only DB
  read — no restic call, ever, not even a fallback on a miss; see its doc comment in repo.rs for
  why removing that fallback was itself a fix, not just a simplification: `RepositoriesPage`
  requests this for every repo on mount, and it used to fall through to a live `restic stats` call
  on a cache miss — harmless normally, but "Clear All Cache" wipes every repo's cached stats at
  once, so the very next mount silently refreshed all of them, contradicting stats' manual-only
  design). Nothing runs that a task could represent in any of the three.
- **Continuous background work, not user-bounded** — `cache_warmer`'s `refresh_all_snapshots` tick
  (runs automatically every 60s, forever, per eligible repo, for as long as the app is open). Unlike
  every other wired operation this isn't bounded by a user action, so it was kept off the bus to
  avoid unbounded event volume over a long-running session; revisit deliberately if a future
  consumer needs ambient background activity visible too.

Any new restic-shelling command should go through `OperationCtx` unless it falls in one of those
two categories.

**Frontend scope — five stateful consumers so far (`stats`, `index`'s per-snapshot lifecycle,
`index`'s batch-level progress, the scheduler-backup `activeBackup` row, and `prune`'s
`activePrune` row); everything else still emits into the void.** `src/lib/types.ts`
mirrors the envelope (`TaskEvent`, `TaskKind`, `TaskPhase`,
`TaskOrigin`, `TaskProgress`) so a consumer has a ready-made contract. `ActivityProvider`
(`src/lib/activity.tsx`) subscribes to `task` filtered to `kind: "stats"` — repo stats refreshes
never had a legacy per-operation feed (the page always updated straight from the command's
promise return), so this was the first case with no existing detail feed to duplicate or
choreograph around. The subscription is deliberately **lifecycle-only, and text-free**: it tracks
`operationId → repoId` across `started`/`finished`/`failed`/`cancelled` to drive both an
in-flight spinner (`statsRefreshing`) and a plain boolean "last attempt failed" marker
(`statsFailed`, cleared the moment a new attempt starts or a later one succeeds) — both
surfaced in `ActivityPanel` and read directly by `RepositoriesPage`. No error *message* is ever
carried, stored, or shown; a manual refresh only needs to tell the user "that didn't work," not
restic's specific reason, so the marker is a `Set<repoId>`, never a `Map<repoId, string>`. This
is also why `fetch_and_cache_stats` (`repo.rs`) creates its `OperationCtx` **before** validating
the master key or resolving the repo, with every fallible step explicitly calling
`task_ctx.failed(e)` rather than relying on `?` — the frontend marker has no fallback to the
invoke promise's own rejection, so every failure path must reliably reach the bus (previously,
auth/repo-lookup failures emitted no task event at all, and a `parse_stats_json` failure fell
through to `OperationCtx`'s `Drop` backstop instead of an explicit call).

`index` is the second consumer, and the first case of a **legacy event fully retired** rather than
added alongside — the old `index:done` event (emitted by `index_snapshot`, `index_snapshots_batch`,
and `cache_warmer`'s auto-indexer) was removed outright once its four listeners
(`activity.tsx`, `SnapshotsPage`, `SearchPage`, `RepoSearchPage`) were ported to `task`, since the
envelope already carried a strict superset of its payload (`snapshotId`→`targetId`, `repoId`,
`success`→`phase`). Each listener filters to `kind === "index"` and a terminal phase
(`"finished"`/`"failed"`); `activity.tsx` uses it as a pure lifecycle trigger for `refreshIndexing`
(same as its unmigrated `snapshots:refreshed` listener), while the three page-level listeners read
`targetId`/`phase` directly to drive per-row index-status maps and the "Index All" batch progress
UI — a case where, unlike `stats`, the event payload itself (not just its lifecycle) is consumed.
`snapshots:refreshed` remains on the legacy path deliberately: it's `cache_warmer`'s
`refresh_all_snapshots` tick, which is excluded from the `task` bus entirely (see the coverage
exclusions above).

`index_snapshots_batch` ("Index All") additionally emits a **batch-level** `task` op alongside its
per-snapshot ones — `kind: "index"`, `origin: "manual"`, but with **no `targetId`**, which is the
only thing that distinguishes it from the per-snapshot events on the wire (see `browse.rs`'s
`index_snapshots_batch` doc comment). It reports `phase: "progress"` with `itemsDone`/`itemsTotal`
as the batch advances. Each batch owns its own cancel flag + task slot rather than sharing one
across every batch (see `IndexHandle::batches`, `cache.rs`), so `cancel_index_batch(operation_id)`
targets exactly one running batch. `ActivityProvider` (`activity.tsx`) tracks these ops via
`reduceIndexBatches` — its first case of reading `progress` off the bus rather than treating it
purely as a lifecycle signal — as a `Map<operationId, ActiveIndexBatch>` (the same shape
`StatsOpsState` already uses for concurrent stats refreshes), exposed as `activeIndexBatches: []`;
`ActivityPanel` renders **one row per active batch**, each a determinate "X / N snapshots" bar with
its own Stop button in Active Tasks. Each event only carries `repoId`, so `ActivityProvider`
separately resolves display names via a single `listRepos()` call covering the whole set of
currently-active batches' repoIds (`indexBatchRepoNames`, re-fetched whenever that set changes) —
the same by-id lookup `loadUpcoming` does for plan names, just async since a batch can start at any
time; falls back to a repo-less label per batch if the lookup fails or that repo was deleted
mid-batch. This is deliberately the one exception to "restore/copy/mirror/manual backup/prune
already have their own progress modals and are intentionally excluded" (see `ActivityPanel.tsx`'s
header comment): "Index All"'s modal — `RepoSearchPage`, and independently `RepositoriesPage`'s
context-menu equivalent — is explicitly dismissible while its batch keeps running, so unlike those
other modals it needs a way to stay visible and cancellable after the modal closes — each page's own
Stop button captures its batch's `operationId` from the same `started` task event (see the page's
`task` listener) so it targets only its own batch, independent of any other batch running elsewhere
(including one started from the *other* page — each page's `getActiveIndexBatch` call adopts an
already-running batch for the same repo instead of rejecting/duplicating). The existing per-snapshot
listeners (`SnapshotsPage`/`SearchPage`/`RepoSearchPage`) already guard on `targetId` being set, so they
transparently ignore every batch-level op with no changes required.

`activeBackup` (the scheduler-triggered backup row in Active Tasks) is the fourth consumer, and the
first case of a **legacy event family fully retired** for a multi-op lifecycle rather than a
single event (`index:done` was one event; this replaced four: `scheduler:backup-started`,
the guarded `backup:progress`, `scheduler:retention-started`, `scheduler:backup-finished`).
`reduceSchedulerBackup` (`activity.tsx`) filters `task` to `origin: "scheduler"` and stitches two
separate ops into one continuous row across a plan's full run: `kind: "backup"` (started/progress/
finished, `targetId` = plan id) for the transfer phase, then `kind: "forget"` (started/finished,
same `targetId`, matched only once the backup op has reached `finished`) for retention — the same
two ops `execute_backup`/`apply_retention` already emit via their own `OperationCtx`s (see
scheduler.rs), just filtered and correlated on the frontend rather than the backend re-emitting a
combined signal. A plan with no retention configured never gets a `forget` op (`scheduler.rs` only
calls `apply_retention` when the plan has ≥1 `keep_*` flag set), so the reducer alone can't decide
when to dismiss that case — it leaves the row sitting with a `backupFinished: true` marker, and a
separate provider effect (keyed on the run and re-checked when `backupFinished` flips) resolves the
plan via `listBackupPlans()` and clears the row once it confirms no retention is coming. That same
lookup also resolves the row's display name — the bus carries only the plan id, not a display name,
same as `indexBatchRepoNames` above — so the row shows plan name only (the legacy events additionally
carried a schedule name that the bus has no equivalent for). This same fix-the-early-return-gap
pattern used by `fetch_and_cache_stats` above was needed in `apply_retention` too, for the identical
reason: it originally did `master_key.get()?`/`db.get_full_repo(...)?` before creating its
`OperationCtx`, so a failure there (e.g. the plan's repo deleted in the narrow window between backup
and retention) skipped emitting any `forget` event at all — with the legacy `scheduler:backup-finished`
retired, nothing else would have cleared the row, leaving it stuck until some unrelated later
scheduled backup happened to displace it. `apply_retention` now creates its `OperationCtx` first and
calls `task_ctx.failed(e)` explicitly on both lookups, closing that gap the same way `repo.rs` already
had for stats.

`activePrune` (the "Pruning repositories" row in Active Tasks) is the fifth consumer, and —
like `index`'s batch-level progress — reads real `progress` off the bus rather than treating it
purely as a lifecycle signal. `reducePrune` (`activity.tsx`) is a single nullable slot, not a
`Map`, since prune is single-in-flight app-wide (`PruneHandle`'s `busy` guard) unlike concurrent
index batches. It covers both `prune_all_repos` (progress-bearing: `itemsDone`/`itemsTotal`/
`label` per repo) and single-repo `prune_repo` (lifecycle-only — `itemsTotal` stays `0`, so the
row renders indeterminate). This is also the first case of a legacy event retired **after** its
one remaining consumer was ported on its own, unprompted by a wider rewrite: the legacy
`prune:progress` event existed solely to feed `SettingsPage`'s "Prune All Repositories" modal
(its progress bar, "Pruning `<repo>` (n of N)…" text, and repo count), which now mirrors the
same `activePrune` state the Activity panel already reads, gated on that modal's own local
`pruning` flag so a concurrent single-repo prune sharing the slot can't overwrite its numbers.
Once ported, the `app.emit("prune:progress", ...)` calls and their `PruneProgress` struct
(`repo.rs`) were deleted outright — the same "retire once ported" treatment as `index:done` and
the `scheduler:*` events, just triggered by a single-consumer migration rather than a four- or
five-listener one.

The *data* (the actual `ResticStats` numbers) never rides the event either — a consumer hears
`finished` and re-reads `get_repo_stats` (a guaranteed cache hit, since `fetch_and_cache_stats`
writes `repo_stats_cache` before it calls `task_ctx.finished()`), rather than widening the
envelope with a result payload. That ordering (cache write before `finished`) is intentional —
it makes "task says finished" provably imply "cache read will see the new value," not just
usually true.

`backup`/`forget` are now consumed too, but only partially — `reduceSchedulerBackup` filters to
`origin: "scheduler"`, so manual/"Run Now" backups and manual retention (`forget_by_plan`,
`run_schedule_now`'s retention call) still emit into the void for Activity-panel purposes; they
already have their own progress modals per the "Restore/copy/mirror/manual backup" exclusion
above. For every other kind (`restore`, `copy`, `mirror`, …) **no stateful frontend code subscribes
to `task`** at all yet — that remains deliberate, not an oversight: a live consumer wired before
there's an actual feature needing it risks the same fate as an earlier, scrapped attempt at this
pattern (over-eager re-renders, a shape that rots before it's ever exercised). `App.tsx`'s dev-only
`console.debug("[task]", ...)` effect still covers those — stateless (never calls `setState`),
gated on `import.meta.env.DEV`, safe to delete. The floor against "emitting into the void" for
the rest is `tasks.rs`'s own test suite (a recording `TaskSink` asserting lifecycle ordering and
the exact camelCase JSON shape) plus the shared TypeScript types keeping the two sides in sync.

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
- `list_snapshots` returns from cache only, via `AppDb::get_snapshots_vec` (rows parsed straight into `Vec<Snapshot>` — no intermediate JSON-string serialize/parse round-trip); `refresh_snapshots` calls restic and updates cache.
- SnapshotsPage: stale-while-revalidate — serve cache immediately (a `load()` effect keyed on `[repoId, repo]`), then a single background refresh (a separate effect gated on a `settingsReady` flag, so it fires exactly once per repo visit with the resolved `remoteAutoRefresh` value rather than twice — once before and once after that setting loads).
- After `run_backup`: new snapshot metadata prepended to cache (no full re-fetch).
- After `forget_by_plan`: full `restic snapshots --json` repopulates cache.
- `remove_repo` cascades to `browse_cache_status`, `browse_cache_files`, `snapshots_cache`, and `repo_stats_cache`, all inside one transaction (all-or-nothing).
- `clear_browse_cache` (Clear All Cache): DELETEs all cache tables, then `PRAGMA wal_checkpoint(TRUNCATE)` + `VACUUM` to reclaim disk space from the WAL and compact the main file.
- `backup_history` is bounded: `log_backup` trims to the newest `BACKUP_HISTORY_LIMIT` (1000) rows after each insert (guarded by a `COUNT(*)` check so a normal insert under the cap skips the DELETE), matching the read limit so the Logs page never loses visible rows. Indexed via `idx_history_started` on `started_at`, which backs both the trim's `ORDER BY` and `list_backup_history`'s.
- Background cache warmer: every 60s, snapshot metadata is refreshed for all eligible repos (always). File indexing (browse_cache) is pre-populated in the background only when `auto_indexing` is enabled. Both phases skip remote repos unless `remote_auto_refresh` is on. `refresh_all_snapshots` hashes each repo's raw `restic snapshots --json` output and skips the `set_snapshots` DELETE+re-INSERT (and the `snapshots:refreshed` emit) when it's unchanged since the last tick — avoids rewriting `snapshots_cache` every minute for the common case of no new snapshots. The per-repo hash is held in-memory only (`cache_warmer::spawn`'s local `HashMap`), so it resets on app restart, which just costs one extra write on the first tick.
- `AppDb` holds one `Mutex<Connection>` — every command using `AppDb` shares that single lock, so a slow synchronous query (e.g. a repo-wide `LIKE` search over hundreds of thousands of cached file rows, ~1s+) held on a core async-runtime thread starves *every other* command that also touches `AppDb` (snapshot refreshes, index-status polling, the cache warmer tick) until it finishes. Any new command doing DB work slow enough to notice should be `async fn` and run the actual query via `tauri::async_runtime::spawn_blocking` (see `search_snapshot_files`/`search_repo_files`, or the existing `index_snapshot`/`restore_snapshot`) so it occupies a blocking-pool thread instead of a scarce core worker thread.

## Intentional Designs (do not "optimize" these)

These have come up as apparent inefficiencies during codebase audits and were deliberately kept
as-is. Don't re-flag or "fix" them without understanding why first:

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

## Versioning

`src-tauri/tauri.conf.json`'s `version` field is the **only** version that matters — it's a literal
semver string, not a path, so Tauri reads it directly and never falls back to `package.json` (per
`@tauri-apps/cli`'s own config schema, that fallback only applies when `version` is set to a path
pointing at a `package.json` file, or omitted entirely, in which case it falls back to
`Cargo.toml`). The in-app version shown in `Sidebar.tsx` comes from `@tauri-apps/api/app`'s
`getVersion()`, which also resolves from `tauri.conf.json`. On a release bump, only
`tauri.conf.json` needs to change.

`package.json` and `package-lock.json` deliberately carry **no** `version` field — there's nothing
in the toolchain or CI that reads it (confirmed: neither workflow in `.github/workflows/` nor any
frontend/backend code references it), so there's nothing to keep in sync. Don't add one back.

`src-tauri/Cargo.toml`'s `version` is similarly deliberately left at `0.0.0` — that crate version is
unused (Tauri does not read it for the app version), and `0.0.0` signals "not the source of truth"
to avoid confusion; do not bump it.

## Build Profile

`src-tauri/Cargo.toml` sets `[profile.release]`: `strip = true`, `lto = true`, `codegen-units = 1` — a smaller/faster release binary at the cost of longer compile time (accepted; CI/local dev builds are unaffected since this only applies to `--release`). `opt-level` is left at the release default (`3`). `panic = "abort"` is deliberately **not** set — see Intentional Designs.

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
- Rust unit tests use `#[cfg(test)]` modules in `scheduler.rs`, `cache_warmer.rs`, and `commands/{cache,crypto,repo,repo_locks,snapshot,schedule,transfer,browse}.rs`.
- CI (`.github/workflows/test.yml`) runs on every push that isn't a `v*` tag and on PRs.

```bash
npm run typecheck   # tsc --noEmit (tsconfig has strict + noUnusedLocals/Parameters)
npm run lint         # eslint src (react-hooks rules only, see below)
npm run lint:rust    # cargo clippy --all-targets -- -D warnings
npm run lint:all     # both of the above
npm run test:vite   # frontend tests only
npm run test:rust   # Rust tests only (cargo test)
npm run test:all    # both
```

Linting is deliberately narrow and **not wired into CI** — it's a local-only gate you're expected
to run yourself after touching hook logic or Rust code, not a merge blocker. `eslint.config.js`
enables only `eslint-plugin-react-hooks` (`rules-of-hooks` + `exhaustive-deps`) — no
`typescript-eslint` rule sets, no stylistic rules — because `npm run typecheck` already covers
type errors and stylistic linting adds churn without preventing the regressions this project
actually sees. `npm run lint:rust` runs `cargo clippy` with `-D warnings`; the few call sites that
can't reasonably shrink (`#[tauri::command]`s with many parameters, one intentionally
fire-and-forget `spawn_blocking` unlock) carry a targeted `#[allow(clippy::...)]` with a comment,
matching the pre-existing pattern in `cache.rs`. Neither linter catches this project's actual
biggest regression risk — the concurrency/ordering invariants documented in the Concurrency and
Restic Integration sections above (`RepoLocks` ordering, `busy` flags, cancel-path races); those
remain the job of tests and review, not static analysis.

## Running the App

```bash
npm install
npm run tauri dev   # requires Rust installed
npm run tauri build # distributable
npm run clean       # remove dist/ and src-tauri/target/
```
