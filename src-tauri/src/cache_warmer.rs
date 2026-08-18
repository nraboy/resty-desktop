use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{Emitter, Manager};

use crate::commands::browse::run_full_index;
use crate::commands::cache::{run_cleanup, AppDb, CleanupHandle, IndexHandle, MasterKey};
use crate::commands::repo::run_restic_with_path;
use crate::commands::repo_locks::RepoLocks;
use crate::tasks::{OperationCtx, TaskKind, TaskOrigin};

const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

// 60s ticks -> every 5th tick is ~5 minutes. Orphans are never urgent, so a
// modest, non-configurable cadence is preferred over a Settings toggle — see
// CLAUDE.md's "Settled decisions".
const CLEANUP_EVERY_N_TICKS: u32 = 5;

pub(crate) fn is_remote(path: &str) -> bool {
    REMOTE_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

pub fn spawn(app: tauri::AppHandle) {
    let running = Arc::new(AtomicBool::new(false));
    // Tracks the last-seen `restic snapshots --json` hash per repo so
    // refresh_all_snapshots can skip the cache rewrite when nothing changed.
    let mut snapshot_hashes: HashMap<String, u64> = HashMap::new();

    tauri::async_runtime::spawn(async move {
        // Short initial delay to let the app finish initialising before the first sweep.
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        refresh_all_snapshots(&app, &mut snapshot_hashes).await;
        trigger_sweep(&app, &running);

        let mut tick: u32 = 0;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            refresh_all_snapshots(&app, &mut snapshot_hashes).await;
            trigger_sweep(&app, &running);
            tick = tick.wrapping_add(1);
            if tick.is_multiple_of(CLEANUP_EVERY_N_TICKS) {
                maybe_run_cleanup(&app).await;
            }
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

/// Runs the same routine as the "Clean Orphaned Data" button, on the warmer's tick —
/// see `run_cleanup`'s doc comment for the mechanism. Deliberately bails out rather than
/// queueing in every skip case below: orphans are never urgent, and there is always
/// another tick five minutes away.
///
/// Safe by construction, not by a staleness gate: `set_snapshots`/`remove_snapshot_from_
/// cache`/etc. can no longer manufacture an artificially-empty `snapshots_cache` (the bug
/// that made automatic cleanup unsafe the first time — see docs/decisions.md), so a repo
/// that hasn't refreshed yet just has a stale-but-present cache, which can only cause
/// under-cleanup — self-healed by the next sweep, automatic or manual. The one thing that
/// still needs an explicit check is the lock: cleanup shells out to nothing and needs no
/// master key, so unlike `refresh_all_snapshots` it will not stop on its own when locked.
///
/// The actual drain (`run_cleanup`) is spawned detached, not awaited, so a large backlog
/// (100K+ rows observed in practice, draining over multiple batches) never stalls this
/// loop's own 60s snapshot refresh and index sweep — the same reason `trigger_sweep`
/// above spawns detached rather than being awaited inline. Safe to overlap with a manual
/// click or a still-draining previous tick: `run_cleanup`'s `CleanupHandle.busy`
/// `compare_exchange` is checked from *inside* that call, so whichever run got there
/// first proceeds and the other's call returns `Err` immediately — nothing queues.
async fn maybe_run_cleanup(app: &tauri::AppHandle) {
    if app.state::<MasterKey>().get().is_err() {
        return; // locked — nothing should touch the DB automatically until unlock
    }
    // Cheap pre-check to avoid spawning a task we'd immediately abandon — not the real
    // guard, which is run_cleanup's own busy compare_exchange (see doc comment above).
    if app.state::<CleanupHandle>().busy.load(Ordering::SeqCst) {
        return;
    }
    let app2 = app.clone();
    let has_work = tauri::async_runtime::spawn_blocking(move || app2.state::<AppDb>().has_cleanup_work()).await;
    if !matches!(has_work, Ok(Ok(true))) {
        return; // nothing to do, or the probe failed — either way, wait for the next tick
    }
    let app_for_cleanup = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_cleanup(app_for_cleanup, TaskOrigin::Background).await;
    });
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
    use super::is_remote;

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
}
