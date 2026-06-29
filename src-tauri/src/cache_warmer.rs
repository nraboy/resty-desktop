use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{Emitter, Manager};

use crate::commands::browse::run_full_index;
use crate::commands::cache::{AppDb, MasterKey};

const REMOTE_PREFIXES: &[&str] = &["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

fn is_remote(path: &str) -> bool {
    REMOTE_PREFIXES.iter().any(|p| path.starts_with(p))
}

pub fn spawn(app: tauri::AppHandle) {
    let running = Arc::new(AtomicBool::new(false));

    tauri::async_runtime::spawn(async move {
        // Short initial delay to let the app finish initialising before the first sweep.
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        trigger_sweep(&app, &running);

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            trigger_sweep(&app, &running);
        }
    });
}

/// Starts a sweep if one is not already running. The sweep continuously
/// indexes uncached snapshots one at a time with no delay between them,
/// stopping only when there is nothing left to index.
fn trigger_sweep(app: &tauri::AppHandle, running: &Arc<AtomicBool>) {
    if running.swap(true, Ordering::SeqCst) {
        return; // sweep already in progress
    }

    let app = app.clone();
    let running = Arc::clone(running);

    tauri::async_runtime::spawn(async move {
        loop {
            match index_next(&app).await {
                SweepResult::Indexed => continue, // immediately try the next one
                SweepResult::NothingLeft | SweepResult::Locked => break,
            }
        }
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

    if master_key.get().is_err() {
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

    let ok = tauri::async_runtime::spawn_blocking(move || {
        let db_inner = app2.state::<AppDb>();
        let result = run_full_index(&db_inner, &repo_id, &repo, &snapshot_id, &restic_path);
        if result.is_err() {
            let _ = db_inner.set_browse_status(&repo_id, &snapshot_id, "pending");
        }
        result.is_ok()
    })
    .await
    .unwrap_or(false);

    let _ = app.emit("index:done", serde_json::json!({
        "snapshotId": emit_snapshot_id,
        "repoId": emit_repo_id,
        "success": ok,
    }));

    if ok { SweepResult::Indexed } else { SweepResult::NothingLeft }
}
