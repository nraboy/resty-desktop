use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{Emitter, Manager};

use crate::commands::browse::run_full_index;
use crate::commands::cache::{AppDb, IndexHandle, MasterKey};
use crate::commands::repo::run_restic_with_path;
use crate::commands::repo_locks::RepoLocks;

const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

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

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            refresh_all_snapshots(&app, &mut snapshot_hashes).await;
            trigger_sweep(&app, &running);
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

    if app.state::<IndexHandle>().manual_active.load(Ordering::SeqCst) {
        return; // manual indexing in progress — yield this tick
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

    if index_handle.manual_active.load(Ordering::SeqCst) {
        // Manual indexing (single-snapshot or "Index All") is active — stop
        // the sweep loop cleanly; it will restart on a later tick.
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

    let emit_repo_id = repo_id.clone();
    let emit_snapshot_id = snapshot_id.clone();

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

    let _ = app.emit("index:done", serde_json::json!({
        "snapshotId": emit_snapshot_id,
        "repoId": emit_repo_id,
        "success": ok,
    }));

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
