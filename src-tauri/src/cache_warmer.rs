use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{Emitter, Manager};

use crate::commands::browse::run_full_index;
use crate::commands::cache::{
    run_drain_loop, AppDb, CleanupHandle, DrainOutcome, IndexHandle, MasterKey,
    CLEANUP_RECONCILE_EVERY_N_TICKS, CLEANUP_VISIBILITY_THRESHOLD_ROWS,
};
use crate::commands::repo::run_restic_with_path;
use crate::commands::repo_locks::RepoLocks;
use crate::tasks::{OperationCtx, TaskKind, TaskOrigin, TaskProgress};

const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

pub(crate) fn is_remote(path: &str) -> bool {
    REMOTE_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

/// Whether an automatic drain of `total` pending rows should be surfaced on the `task`
/// bus. Extracted as a pure function so the threshold behavior is directly unit-testable
/// without needing an `AppHandle` — see `CLEANUP_VISIBILITY_THRESHOLD_ROWS`'s doc comment.
pub(crate) fn is_cleanup_visible(total: u64) -> bool {
    total > CLEANUP_VISIBILITY_THRESHOLD_ROWS
}

/// Whether this tick should also run the periodic full `mark_orphans` backstop sweep, on
/// top of diff-based marking. Extracted for the same reason as `is_cleanup_visible`.
pub(crate) fn is_reconcile_tick(tick_count: u64) -> bool {
    tick_count.is_multiple_of(CLEANUP_RECONCILE_EVERY_N_TICKS)
}

pub fn spawn(app: tauri::AppHandle) {
    let running = Arc::new(AtomicBool::new(false));
    // Tracks the last-seen `restic snapshots --json` hash per repo so
    // refresh_all_snapshots can skip the cache rewrite when nothing changed.
    let mut snapshot_hashes: HashMap<String, u64> = HashMap::new();
    // Counts ticks so trigger_cleanup_drain knows when to run its periodic
    // mark_orphans backstop (see CLEANUP_RECONCILE_EVERY_N_TICKS). In-memory only —
    // resets on restart, which just pushes the next backstop sweep out by up to that
    // many ticks; diff-based marking (set_snapshots) is unaffected either way.
    let mut tick_count: u64 = 0;

    tauri::async_runtime::spawn(async move {
        // Short initial delay to let the app finish initialising before the first sweep.
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        refresh_all_snapshots(&app, &mut snapshot_hashes).await;
        trigger_sweep(&app, &running);
        trigger_cleanup_drain(&app, tick_count);

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            tick_count += 1;
            refresh_all_snapshots(&app, &mut snapshot_hashes).await;
            trigger_sweep(&app, &running);
            // After refresh_all_snapshots so this tick's own set_snapshots diffs (if any
            // repo's list changed) are included in what gets drained. Fire-and-forget,
            // same shape as trigger_sweep — never awaited inline, so a slow drain can
            // never delay the next tick's refresh.
            trigger_cleanup_drain(&app, tick_count);
        }
    });
}

/// Refreshes the snapshots cache for all eligible repos. Always runs on every
/// 60s tick regardless of the auto_indexing setting; respects remote_auto_refresh.
/// Skips the cache rewrite (and the `snapshots:refreshed` emit) for a repo when its
/// `restic snapshots --json` output hasn't changed since the last tick, tracked via
/// `snapshot_hashes` — avoids a full DELETE+re-INSERT every minute for the common
/// case of an unchanged snapshot list.
async fn refresh_all_snapshots(app: &tauri::AppHandle, snapshot_hashes: &mut HashMap<String, u64>) {
    let db = app.state::<AppDb>();
    let master_key = app.state::<MasterKey>();
    let repo_locks = app.state::<RepoLocks>();

    let key = match master_key.get() {
        Ok(k) => k,
        Err(_) => return,
    };

    let remote_auto_refresh = db
        .get_setting("remote_auto_refresh", "false")
        .unwrap_or_else(|_| "false".to_string())
        == "true";

    let repos = match db.list_repos() {
        Ok(r) => r,
        Err(_) => return,
    };

    // Fetched once per sweep rather than per repo — it's the same settings lookup every time.
    let restic_path = crate::commands::get_restic_path(&db);

    for repo_meta in repos {
        if !remote_auto_refresh && is_remote(&repo_meta.path) {
            continue;
        }

        let repo_id = repo_meta.id.clone();
        let repo = match db.get_full_repo(&repo_id, &key) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let restic_path = restic_path.clone();
        let app2 = app.clone();

        // `snapshots` is a shared-lock read — register as a reader, held across the
        // spawn_blocking below.
        let _rg = repo_locks.read(&repo.path);
        let result = tauri::async_runtime::spawn_blocking(move || {
            run_restic_with_path(&repo, vec!["snapshots", "--json"], &restic_path)
        })
        .await;

        if let Ok(Ok(json)) = result {
            let new_hash = hash_str(&json);
            let db2 = app2.state::<AppDb>();
            let unchanged = snapshot_hashes.get(&repo_id) == Some(&new_hash)
                && db2.has_cached_snapshots(&repo_id).unwrap_or(false);
            if unchanged {
                continue; // Unchanged since last tick and cache still populated — skip rewrite/emit.
            }

            if db2.set_snapshots(&repo_id, &json).is_ok() {
                snapshot_hashes.insert(repo_id.clone(), new_hash);
                let _ = app2.emit("snapshots:refreshed", serde_json::json!({ "repoId": repo_id }));
            }
        }
    }
}

/// Starts a file-indexing sweep if auto_indexing is enabled and one is not
/// already running. The sweep continuously indexes uncached snapshots one at a
/// time with no delay between them, stopping only when there is nothing left.
/// Does not start while manual indexing (single-snapshot or "Index All") is
/// active — see `IndexHandle::manual_active`; the sweep resumes on a later
/// tick once manual indexing finishes.
fn trigger_sweep(app: &tauri::AppHandle, running: &Arc<AtomicBool>) {
    let db = app.state::<AppDb>();
    let auto_indexing = db
        .get_setting("auto_indexing", "false")
        .unwrap_or_else(|_| "false".to_string())
        == "true";

    if !auto_indexing {
        return;
    }

    if app.state::<IndexHandle>().manual_active.load(Ordering::SeqCst) != 0 {
        return; // manual indexing in progress (or queued) — yield this tick
    }

    if running.swap(true, Ordering::SeqCst) {
        return; // sweep already in progress
    }

    let app = app.clone();
    let running = Arc::clone(running);

    tauri::async_runtime::spawn(async move {
        // Loop until nothing's left to index (or we yield to manual indexing).
        while let SweepResult::Indexed = index_next(&app).await {}
        running.store(false, Ordering::SeqCst);
    });
}

/// Fire-and-forget entry point for the automatic orphan drain — spawns `run_cleanup_drain`
/// and returns immediately, unconditionally (the decision about whether there's anything to
/// do, and whether `CleanupHandle` is free, lives inside `run_cleanup_drain` itself, not
/// here). Same shape as `trigger_sweep`: never awaited inline, so a slow drain can't delay
/// the next tick's `refresh_all_snapshots`.
fn trigger_cleanup_drain(app: &tauri::AppHandle, tick_count: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_cleanup_drain(&app, tick_count).await;
    });
}

/// Drains whatever `set_snapshots`'s diff-based marking has already identified as orphaned
/// (see its doc comment — that marking is safe on every call, by construction, no staleness
/// guard needed), plus a periodic `mark_orphans` backstop for anything that doesn't flow
/// through a diff-aware `set_snapshots` call (see `all_repos_have_cached_snapshots`'s doc
/// comment for why *that* backstop, unlike the diff, genuinely needs its own staleness
/// guard, and for the concrete failure it closes).
///
/// `CleanupHandle::busy` is claimed only once real pending work is confirmed (not up front,
/// unlike `clean_cache`) — see the comment at that call site. Shares `CleanupHandle` with
/// the manual "Clean Orphaned Data" button (`clean_cache`) rather than using its own
/// reentrancy flag (unlike `trigger_sweep`'s `running`): this is what keeps the frontend's
/// single-slot `ActiveCleanup` state correct — at most one cleanup operation is ever "the"
/// active one on the `task` bus — and means a manual click during an automatic drain fails
/// fast with the same "already running" error `clean_cache` already returns, rather than
/// racing it.
///
/// Needs no master-key check the way `refresh_all_snapshots`/`index_next` do — marking and
/// draining touch no repo secrets, so the mechanics run regardless of lock state. That is
/// *not* the same claim as "safe regardless of lock state": the backstop's own
/// `all_repos_have_cached_snapshots` gate is what actually makes running before unlock safe,
/// by refusing to reconcile against data it can't currently trust — not the absence of a
/// lock check here.
async fn run_cleanup_drain(app: &tauri::AppHandle, tick_count: u64) {
    let cleanup_handle = app.state::<CleanupHandle>();

    // Periodic backstop, beyond diff-based marking: catches anything that doesn't flow
    // through a diff-aware set_snapshots call — `AppDb::evict_snapshots` empties a repo's
    // entire `snapshots_cache` with no replacement, called both unconditionally on success
    // (delete_snapshot; copy_snapshot's/mirror_repo's destination repo) and as a
    // re-fetch-failure fallback (execute_backup, apply_retention) — either way leaving that
    // repo in exactly that state until some later refresh repopulates it.
    //
    // Unlike set_snapshots's diff (safe on every call, by construction — see its doc
    // comment), a full-table reconciliation across every repo's snapshots_cache at once
    // genuinely depends on all of them reflecting current truth, not just whichever repo
    // prompted this tick — without a gate, it would treat a repo left empty by
    // evict_snapshots as having zero live snapshots and mark its entire indexed history
    // orphaned. `mark_orphans_if_all_repos_fresh` (cache.rs) is that gate, checked and
    // applied atomically in one transaction — not a separate check-then-call here, which
    // would leave a real TOCTOU window given AppDb's one shared connection releases its
    // lock between separate method calls (see that method's doc comment). Reachable
    // through ordinary use (delete a snapshot, quit before the next refresh; a remote repo
    // with remote_auto_refresh off, which refresh_all_snapshots skips indefinitely) and,
    // since this tick runs from launch regardless of lock state, possibly within 10
    // seconds of opening the app, before the password screen is even dismissed. This is
    // exactly the evict_snapshots staleness race the diff-based redesign was built to
    // eliminate — the backstop, being a scan rather than a diff, would reintroduce it
    // unless gated. See docs/decisions.md.
    //
    // Errors ignored, same fire-and-forget tolerance as the rest of this file's background
    // sweeps; a skipped or failed reconciliation this tick just means it's retried next time
    // the counter wraps around.
    if is_reconcile_tick(tick_count) {
        let app2 = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            app2.state::<AppDb>().mark_orphans_if_all_repos_fresh()
        })
        .await;
    }

    // Cheap bail-out before paying for pending_orphan_row_count's COUNT(*), and — just as
    // importantly — before ever touching CleanupHandle::busy at all: on the common tick
    // where nothing is marked, this function must never look like it's "running" to a
    // concurrent manual click. Deferring the busy claim until real work is confirmed below
    // narrows (though doesn't eliminate — see docs/decisions.md) the window where a manual
    // click could fail with "already running" against an automatic drain the Activity
    // panel never showed, because it never had anything to show.
    let app2 = app.clone();
    let has_pending = tauri::async_runtime::spawn_blocking(move || {
        app2.state::<AppDb>().has_pending_orphans()
    })
    .await
    .unwrap_or(Ok(false))
    .unwrap_or(false);
    if !has_pending {
        return;
    }

    if cleanup_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // a manual run, or a still-draining previous tick, owns the handle
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&cleanup_handle.busy);
    cleanup_handle.cancelled.store(false, Ordering::SeqCst);

    let app2 = app.clone();
    let total = match tauri::async_runtime::spawn_blocking(move || {
        app2.state::<AppDb>().pending_orphan_row_count()
    })
    .await
    {
        Ok(Ok(t)) => t,
        _ => return,
    };

    // Visibility decided once, up front, from the known total — not lazily mid-loop.
    // Steady-state (a retention run's worth of orphans, typically a few thousand rows)
    // stays silent; only a genuine catch-up drain shows in the Activity panel. This is
    // also what keeps this tick off the documented "unbounded continuous background work"
    // bus exclusion the same way clean_cache already is — see CLAUDE.md.
    let visible = is_cleanup_visible(total);
    let task_ctx = if visible {
        // repo_id is deliberately "" — cleanup is app-wide, not scoped to one repo. See
        // TaskKind::Cleanup's doc comment.
        let ctx = OperationCtx::new(
            app.clone(),
            TaskKind::Cleanup,
            String::new(),
            None,
            TaskOrigin::Background,
            Some(cleanup_handle.current_task.clone()),
        );
        // Same reason clean_cache emits immediately after construction: OperationCtx::new's
        // Started event always carries progress: None, but the total is already known.
        ctx.progress_emitter().emit(TaskProgress {
            items_done: Some(0),
            items_total: Some(total),
            ..Default::default()
        });
        Some(ctx)
    } else {
        None
    };

    // No batch cap — runs to genuine completion in this one call, same as the manual
    // button. There used to be one (CLEANUP_MAX_BATCHES_PER_TICK, removed), justified as
    // "so a slow drain can't delay the next tick's refresh" — that reasoning was wrong.
    // trigger_cleanup_drain is fire-and-forget: it spawns this function as its own task
    // and returns immediately, so the tick loop's next `sleep(60s)` is never blocked by
    // however long this runs. Capping it bought nothing but a correctness bug: a capped
    // run whose backlog wasn't fully drained still reported DrainOutcome::BatchLimitReached,
    // which was mapped to ctx.finished() below — telling the Activity panel the cleanup was
    // done while a large remainder (confirmed in practice: over a million rows) was still
    // queued for the next tick. Draining to completion here removes the false-completion
    // signal at its root instead of trying to represent "still working, paused until next
    // tick" through TaskPhase, which has no such state.
    //
    // DB errors/join failures are swallowed inside run_drain_loop's Error outcome below —
    // same fire-and-forget tolerance as the rest of this file: retried next tick.
    let progress = task_ctx.as_ref().map(|ctx| ctx.progress_emitter());
    // The row count itself is only discarded here, not lost: when task_ctx is Some, every
    // batch's running total is already on the task bus via progress emission below, and
    // when it's None (an invisible, sub-threshold drain) staying silent is the deliberate
    // design — see CLEANUP_VISIBILITY_THRESHOLD_ROWS's doc comment. Nothing else in this
    // file logs its background sweeps to a file either; the manual button's own return
    // value remains the place a user-visible total is ever reported.
    let (_removed, outcome) =
        run_drain_loop(app, &cleanup_handle, progress.as_ref(), 0, total, None).await;

    if let Some(ctx) = task_ctx {
        match outcome {
            DrainOutcome::Cancelled => ctx.cancelled(),
            DrainOutcome::Finished => ctx.finished(),
            DrainOutcome::Error(e) => ctx.failed(e),
            // Structurally unreachable: max_batches is hardcoded None a few lines above,
            // so run_drain_loop can never produce this outcome here. Deliberately NOT
            // folded into the Finished arm — that mapping is exactly the bug that caused
            // a large backlog to be reported as "done" when only one tick's worth had
            // drained (see the comment above the run_drain_loop call). If this ever
            // panics, it means someone reintroduced a cap without also reintroducing a
            // correct way to represent "still working, paused until next tick" — fix
            // that properly rather than restoring the old mapping.
            DrainOutcome::BatchLimitReached => unreachable!(
                "run_cleanup_drain calls run_drain_loop with max_batches: None"
            ),
        }
    }
}

enum SweepResult {
    Indexed,
    NothingLeft,
    Locked,
}

/// Find the next uncached snapshot, index it, and return whether work was done.
async fn index_next(app: &tauri::AppHandle) -> SweepResult {
    let db = app.state::<AppDb>();
    let master_key = app.state::<MasterKey>();
    let index_handle = app.state::<IndexHandle>();

    if master_key.get().is_err() {
        return SweepResult::Locked;
    }

    if index_handle.manual_active.load(Ordering::SeqCst) != 0 {
        // Manual indexing (single-snapshot or "Index All", including a batch
        // merely queued for its turn) is active — stop the sweep loop cleanly;
        // it will restart on a later tick.
        return SweepResult::Locked;
    }

    let remote_auto_refresh = db
        .get_setting("remote_auto_refresh", "false")
        .unwrap_or_else(|_| "false".to_string())
        == "true";

    let repos = match db.list_repos() {
        Ok(r) => r,
        Err(_) => return SweepResult::NothingLeft,
    };

    let eligible_repo_ids: Vec<String> = repos
        .into_iter()
        .filter(|r| remote_auto_refresh || !is_remote(&r.path))
        .map(|r| r.id)
        .collect();

    let (repo_id, snapshot_id) = match db.get_next_unindexed_snapshot(&eligible_repo_ids) {
        Ok(Some(t)) => t,
        _ => return SweepResult::NothingLeft,
    };

    if db
        .set_browse_status(&repo_id, &snapshot_id, "in_progress")
        .is_err()
    {
        return SweepResult::NothingLeft;
    }

    let key = match master_key.get() {
        Ok(k) => k,
        Err(_) => {
            let _ = db.set_browse_status(&repo_id, &snapshot_id, "pending");
            return SweepResult::Locked;
        }
    };

    let repo = match db.get_full_repo(&repo_id, &key) {
        Ok(r) => r,
        Err(_) => {
            let _ = db.set_browse_status(&repo_id, &snapshot_id, "pending");
            return SweepResult::NothingLeft;
        }
    };

    let restic_path = crate::commands::get_restic_path(&db);
    let app2 = app.clone();

    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Index,
        repo_id.clone(),
        Some(snapshot_id.clone()),
        // Real background work — no user action triggered this tick.
        TaskOrigin::Background,
        None,
    );

    // Held across the spawn_blocking call so this can never overlap with a
    // manual index — see IndexHandle::gate.
    let _permit = index_handle.gate.lock().await;
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let db_inner = app2.state::<AppDb>();
        let repo_locks_inner = app2.state::<RepoLocks>();
        let result = run_full_index(&db_inner, &repo_locks_inner, &repo_id, &repo, &snapshot_id, &restic_path);
        if result.is_err() {
            let _ = db_inner.set_browse_status(&repo_id, &snapshot_id, "pending");
        }
        result.is_ok()
    })
    .await
    .unwrap_or(false);
    drop(_permit);

    if ok {
        task_ctx.finished();
    } else {
        task_ctx.failed("Indexing failed");
    }

    if ok { SweepResult::Indexed } else { SweepResult::NothingLeft }
}

#[cfg(test)]
mod tests {
    use super::{is_cleanup_visible, is_reconcile_tick, is_remote};
    use crate::commands::cache::{CLEANUP_RECONCILE_EVERY_N_TICKS, CLEANUP_VISIBILITY_THRESHOLD_ROWS};

    #[test]
    fn is_remote_recognizes_every_remote_prefix() {
        for (path, label) in [
            ("s3:bucket/path", "s3"),
            ("sftp:user@host:/repo", "sftp"),
            ("rest:https://host/repo", "rest"),
            ("azure:container:/repo", "azure"),
            ("gs:bucket:/repo", "gs"),
            ("b2:bucket:/repo", "b2"),
            ("rclone:remote:/repo", "rclone"),
        ] {
            assert!(is_remote(path), "expected {label} path to be remote: {path}");
        }
    }

    #[test]
    fn is_remote_false_for_local_paths() {
        assert!(!is_remote("/Users/nic/repos/backup"));
        assert!(!is_remote(r"C:\repos\backup"));
        assert!(!is_remote("relative/repo/path"));
    }

    #[test]
    fn cleanup_visibility_threshold_is_exclusive() {
        assert!(!is_cleanup_visible(0));
        assert!(!is_cleanup_visible(CLEANUP_VISIBILITY_THRESHOLD_ROWS));
        assert!(is_cleanup_visible(CLEANUP_VISIBILITY_THRESHOLD_ROWS + 1));
        assert!(is_cleanup_visible(u64::MAX));
    }

    #[test]
    fn reconcile_tick_fires_on_tick_zero_and_every_n_after() {
        // Tick 0 (the very first tick) always reconciles — same "no waiting for the first
        // backstop" reasoning as the initial refresh_all_snapshots call in `spawn`.
        assert!(is_reconcile_tick(0));
        assert!(!is_reconcile_tick(1));
        assert!(!is_reconcile_tick(CLEANUP_RECONCILE_EVERY_N_TICKS - 1));
        assert!(is_reconcile_tick(CLEANUP_RECONCILE_EVERY_N_TICKS));
        assert!(is_reconcile_tick(CLEANUP_RECONCILE_EVERY_N_TICKS * 2));
    }
}
