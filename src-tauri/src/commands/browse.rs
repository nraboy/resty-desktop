use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, MasterKey, RestoreHandle};
use super::repo::{run_restic_blocking, run_restic_with_path};
use super::repo_locks::RepoLocks;
use super::snapshot::validate_snapshot_id;
use super::NoConsole;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub size: Option<u64>,
    pub mtime: Option<String>,
    pub mode: Option<u32>,
}

/// A file match from a repo-wide search, attributed to the (newest) snapshot
/// containing it so the frontend can open the correct BrowsePage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFileHit {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub size: Option<u64>,
    pub mtime: Option<String>,
    pub mode: Option<u32>,
    pub snapshot_id: String,
    pub snapshot_short_id: String,
}

fn is_direct_child(entry_path: &str, parent: Option<&str>) -> bool {
    let clean = entry_path.trim_end_matches('/');
    match parent {
        None | Some("") | Some("/") => {
            let mut parts = clean.splitn(3, '/');
            parts.next();
            let name = parts.next().unwrap_or("");
            !name.is_empty() && parts.next().is_none()
        }
        Some(p) => {
            let prefix = format!("{}/", p.trim_end_matches('/'));
            if !clean.starts_with(&prefix) {
                return false;
            }
            let remainder = &clean[prefix.len()..];
            !remainder.is_empty() && !remainder.contains('/')
        }
    }
}

#[tauri::command]
pub async fn list_files(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    path: Option<String>,
) -> Result<Vec<FileEntry>, String> {
    validate_snapshot_id(&snapshot_id)?;
    if let Some(cached) = db.get(&repo_id, &snapshot_id, path.as_deref())? {
        return Ok(cached);
    }

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let mut args = vec!["ls".to_string(), "--json".to_string(), snapshot_id.clone()];
    if let Some(ref p) = path {
        args.push(p.clone());
    }

    let _rg = repo_locks.read(&repo.path);
    let stdout = run_restic_blocking(repo, args, restic_path).await?;

    let mut entries: Vec<FileEntry> = Vec::new();
    for (i, line) in stdout.lines().enumerate() {
        if i == 0 {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<FileEntry>(line) {
            if is_direct_child(&entry.path, path.as_deref()) {
                entries.push(entry);
            }
        }
    }

    let _ = db.set(&snapshot_id, path.as_deref(), &entries);
    Ok(entries)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn restore_path(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    include_path: String,
    target_dir: String,
    strip_leading_path: bool,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    // Nest under a short_id subfolder so restoring the same (or different) file across
    // multiple snapshots into the same target_dir doesn't merge their contents together —
    // matches restore_snapshot's behavior. Skipped when strip_leading_path is set: that
    // mode already collapses the result to a bare basename directly in target_dir, which
    // the caller explicitly asked for.
    let restic_target = if strip_leading_path {
        target_dir.clone()
    } else {
        std::path::Path::new(&target_dir)
            .join(&snapshot_id[..8])
            .to_string_lossy()
            .into_owned()
    };

    // `restore` is a shared-lock read — register as a reader, held across the
    // run_restic_blocking call below for the whole child-process lifetime (same pattern
    // as list_files/restore_snapshot in this file).
    let _rg = repo_locks.read(&repo.path);
    let restic_result = run_restic_blocking(
        repo,
        vec![
            "restore".into(),
            snapshot_id.clone(),
            "--include".into(),
            include_path.clone(),
            "--target".into(),
            restic_target.clone(),
        ],
        restic_path,
    )
    .await
    .map(|_| ());

    // On Windows, restic exits non-zero when it cannot apply platform-specific extended
    // attributes (e.g. macOS EAs) to the restored files. The files are fully restored;
    // only the metadata application fails. Suppress these errors so the caller sees
    // success and the strip logic below can still run.
    #[cfg(target_os = "windows")]
    let restic_result = restic_result.or_else(|e| {
        let only_ea_errors = e.lines().all(|line| {
            let l = line.trim();
            l.is_empty()
                || l.contains("set EA failed")
                || l.contains("extended attribute")
                || l.starts_with("ignoring error")
                || l.starts_with("Fatal: There were")
        });
        if only_ea_errors { Ok(()) } else { Err(e) }
    });

    if strip_leading_path {
        let clean = include_path.trim_start_matches('/');
        let restored_at = std::path::Path::new(&target_dir).join(clean);
        let basename = std::path::Path::new(clean)
            .file_name()
            .ok_or("Cannot determine basename of restore path")?;
        let dest = std::path::Path::new(&target_dir).join(basename);

        if restored_at != dest && restored_at.exists() {
            std::fs::rename(&restored_at, &dest)
                .map_err(|e| format!("Failed to move restored item: {e}"))?;

            // Remove the now-empty ancestor directories up to (but not including) target_dir.
            let target_path = std::path::PathBuf::from(&target_dir);
            let mut cursor = restored_at.parent().map(|p| p.to_path_buf());
            while let Some(p) = cursor {
                if p == target_path {
                    break;
                }
                if std::fs::remove_dir(&p).is_err() {
                    break;
                }
                cursor = p.parent().map(|pp| pp.to_path_buf());
            }
        }
    }

    restic_result
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreProgress {
    pub percent_done: f64,
    pub files_restored: u64,
    pub total_files: u64,
    pub bytes_restored: u64,
    pub total_bytes: u64,
    pub seconds_elapsed: u64,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn restore_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    restore_handle: State<'_, RestoreHandle>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    target_dir: String,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;

    use std::sync::atomic::{AtomicBool, Ordering};

    // Serialize restores: only one may run at a time, so a second concurrent attempt
    // (e.g. the user navigating away mid-restore and starting another) can't clobber
    // the shared `child`/`cancelled` state on the RestoreHandle.
    if restore_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A restore is already in progress".to_string());
    }
    // Released on every exit path — including `?` early returns — so `busy` never
    // gets stuck set.
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&restore_handle.busy);

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let repo_path = repo.path.clone();
    let repo_password = repo.password.clone();

    // Nest under a short_id subfolder so restoring multiple snapshots to the same
    // target_dir doesn't merge their contents together. validate_snapshot_id above
    // guarantees at least 8 hex chars, matching restic's own short_id convention.
    let target_dir = std::path::Path::new(&target_dir)
        .join(&snapshot_id[..8])
        .to_string_lossy()
        .into_owned();

    restore_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&restore_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&restore_handle.cancelled);

    // `restore` is a shared-lock read — register as a reader, held across the
    // spawn_blocking below for the whole child-process lifetime.
    let _rg = repo_locks.read(&repo_path);

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut child = std::process::Command::new(&restic_path)
            .args(["restore", &snapshot_id, "--target", &target_dir, "--json"])
            .env("RESTIC_REPOSITORY", &repo_path)
            .env("RESTIC_PASSWORD", &repo_password)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .no_console()
            .augment_path()
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        let stderr = child.stderr.take().ok_or("failed to capture restic stderr")?;
        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        let stdout = child.stdout.take().ok_or("failed to capture restic stdout")?;

        // Store child so cancel_restore can reach it.
        *child_arc.lock().map_err(|e| e.to_string())? = Some(child);

        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v["message_type"].as_str() == Some("status") {
                    let progress = RestoreProgress {
                        percent_done: v["percent_done"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0),
                        files_restored: v["files_restored"].as_u64().unwrap_or(0),
                        total_files: v["total_files"].as_u64().unwrap_or(0),
                        bytes_restored: v["bytes_restored"].as_u64().unwrap_or(0),
                        total_bytes: v["total_bytes"].as_u64().unwrap_or(0),
                        seconds_elapsed: v["seconds_elapsed"].as_u64().unwrap_or(0),
                    };
                    let _ = app.emit("restore:progress", &progress);
                }
            }
        }

        // stdout exhausted (process ended or was killed); take child back to call wait().
        let status = match child_arc.lock().map_err(|e| e.to_string())?.take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };
        let stderr_str = stderr_thread.join().unwrap_or_default();

        // A successful exit always wins, even if `cancelled` got set in a race (e.g. Stop
        // clicked just as restic finished, or clicked before the child was stored above) —
        // the restore genuinely completed and must not be reported as cancelled.
        if status.success() {
            return Ok(());
        }

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            // The process was killed via SIGKILL and left a stale lock on the repo.
            // We're already on a blocking thread, so it's safe to unlock inline here —
            // wait() above has already reaped the killed process.
            use super::cache::FullRepository;
            let unlock_repo = FullRepository { path: repo_path.clone(), password: repo_password.clone() };
            let _ = run_restic_with_path(&unlock_repo, vec!["unlock"], &restic_path);
            return Err("cancelled".to_string());
        }

        #[cfg(target_os = "windows")]
        {
            let only_ea_errors = stderr_str.lines().all(|line| {
                let l = line.trim();
                l.is_empty()
                    || l.contains("set EA failed")
                    || l.contains("extended attribute")
                    || l.starts_with("ignoring error")
                    || l.starts_with("Fatal: There were")
            });
            if only_ea_errors {
                return Ok(());
            }
        }
        let msg = stderr_str.trim();
        Err(if msg.is_empty() { "restic restore failed".to_string() } else { msg.to_string() })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn cancel_restore(restore_handle: State<'_, RestoreHandle>) -> Result<(), String> {
    restore_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(ref mut child) = *restore_handle.child.lock().map_err(|e| e.to_string())? {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Shared indexing logic: runs `restic ls --json` for an entire snapshot and
/// bulk-inserts all file entries into `browse_cache_files`. Called by both the
/// manual `index_snapshot` command and the background `cache_warmer`.
pub(crate) fn run_full_index(
    db: &AppDb,
    repo_locks: &RepoLocks,
    repo_id: &str,
    repo: &super::cache::FullRepository,
    snapshot_id: &str,
    restic_path: &str,
) -> Result<(), String> {
    // `ls` is a shared-lock read — register as a reader for the duration of the call.
    let _rg = repo_locks.read(&repo.path);
    let stdout =
        run_restic_with_path(repo, vec!["ls", "--json", snapshot_id], restic_path)?;
    let entries: Vec<FileEntry> = stdout
        .lines()
        .skip(1) // first line is the snapshot summary object
        .filter_map(|line| serde_json::from_str::<FileEntry>(line).ok())
        .collect();
    db.insert_browse_files(snapshot_id, &entries)?;
    db.set_browse_status(repo_id, snapshot_id, "complete")
}

/// Marks `manual_active` for the lifetime of a manual indexing run (single or
/// batch) so the cache_warmer auto-indexer knows to pause. Cleared on drop —
/// created inside the spawned task so it stays set for the whole run and
/// clears on every exit path, including per-snapshot errors.
struct ManualIndexGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl ManualIndexGuard {
    fn new(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Self(flag)
    }
}
impl Drop for ManualIndexGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Manually trigger full indexing for a snapshot. Fire-and-forget: returns
/// immediately and runs the index in the background. Safe to call on remote
/// repos since the user explicitly requested it. Pauses the auto-indexer
/// (via `IndexHandle::manual_active`) for the duration and takes the shared
/// `IndexHandle::gate` around the actual indexing work so it never overlaps
/// with an in-flight auto-indexed snapshot.
#[tauri::command]
pub async fn index_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    index_handle: State<'_, super::cache::IndexHandle>,
    repo_id: String,
    snapshot_id: String,
) -> Result<bool, String> {
    validate_snapshot_id(&snapshot_id)?;

    let status_map = db.get_browse_status(&repo_id)?;
    if matches!(
        status_map.get(&snapshot_id).map(|s| s.as_str()),
        Some("complete") | Some("in_progress")
    ) {
        return Ok(false);
    }

    db.set_browse_status(&repo_id, &snapshot_id, "in_progress")?;

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let manual_active = std::sync::Arc::clone(&index_handle.manual_active);
    let gate = std::sync::Arc::clone(&index_handle.gate);

    tauri::async_runtime::spawn(async move {
        let _guard = ManualIndexGuard::new(manual_active);

        let repo_path = repo.path.clone();
        let repo_pass = repo.password.clone();
        let snap_id = snapshot_id.clone();
        let repo_id2 = repo_id.clone();
        let rp = restic_path.clone();
        let app2 = app.clone();

        let _permit = gate.lock().await;
        let ok = tauri::async_runtime::spawn_blocking(move || {
            let tmp_repo = super::cache::FullRepository {
                path: repo_path,
                password: repo_pass,
            };
            let db_inner = app.state::<AppDb>();
            let repo_locks_inner = app.state::<RepoLocks>();
            run_full_index(&db_inner, &repo_locks_inner, &repo_id2, &tmp_repo, &snap_id, &rp).is_ok()
        })
        .await
        .unwrap_or(false);
        drop(_permit);

        if !ok {
            let _ = app2
                .state::<AppDb>()
                .set_browse_status(&repo_id, &snapshot_id, "pending");
        }

        let _ = app2.emit(
            "index:done",
            serde_json::json!({ "snapshotId": snapshot_id, "repoId": repo_id, "success": ok }),
        );
    });

    Ok(true)
}

/// Sequentially index a batch of snapshots in a single repo ("Index All").
/// Runs one `run_full_index` at a time (bounding memory to a single
/// snapshot's file list — see module docs for the concurrent-indexing crash
/// this replaces), pauses the auto-indexer for the duration via
/// `IndexHandle::manual_active`, and takes `IndexHandle::gate` around each
/// snapshot so it can never overlap with an auto-indexed one. Fire-and-forget:
/// returns immediately; progress is reported per-snapshot via `index:done`
/// (same payload shape as `index_snapshot`), matching what the frontend
/// "Index All" progress UI already listens for. A snapshot that fails to
/// index does not abort the batch — the loop continues to the next one.
/// Cancellable between snapshots via `cancel_index_batch`.
#[tauri::command]
pub async fn index_snapshots_batch(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    index_handle: State<'_, super::cache::IndexHandle>,
    repo_id: String,
    snapshot_ids: Vec<String>,
) -> Result<(), String> {
    for id in &snapshot_ids {
        validate_snapshot_id(id)?;
    }

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let manual_active = std::sync::Arc::clone(&index_handle.manual_active);
    let gate = std::sync::Arc::clone(&index_handle.gate);
    let cancel = std::sync::Arc::clone(&index_handle.cancel);
    cancel.store(false, std::sync::atomic::Ordering::SeqCst);

    tauri::async_runtime::spawn(async move {
        let _guard = ManualIndexGuard::new(manual_active);

        for snapshot_id in snapshot_ids {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            let db_outer = app.state::<AppDb>();
            let status_map = match db_outer.get_browse_status(&repo_id) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if matches!(
                status_map.get(&snapshot_id).map(|s| s.as_str()),
                Some("complete") | Some("in_progress")
            ) {
                continue;
            }
            if db_outer
                .set_browse_status(&repo_id, &snapshot_id, "in_progress")
                .is_err()
            {
                continue;
            }

            let repo_path = repo.path.clone();
            let repo_pass = repo.password.clone();
            let snap_id = snapshot_id.clone();
            let repo_id2 = repo_id.clone();
            let rp = restic_path.clone();
            let app2 = app.clone();

            let _permit = gate.lock().await;
            let ok = tauri::async_runtime::spawn_blocking(move || {
                let tmp_repo = super::cache::FullRepository {
                    path: repo_path,
                    password: repo_pass,
                };
                let db_inner = app2.state::<AppDb>();
                let repo_locks_inner = app2.state::<RepoLocks>();
                run_full_index(&db_inner, &repo_locks_inner, &repo_id2, &tmp_repo, &snap_id, &rp).is_ok()
            })
            .await
            .unwrap_or(false);
            drop(_permit);

            if !ok {
                let _ = app
                    .state::<AppDb>()
                    .set_browse_status(&repo_id, &snapshot_id, "pending");
            }

            let _ = app.emit(
                "index:done",
                serde_json::json!({ "snapshotId": snapshot_id, "repoId": repo_id.clone(), "success": ok }),
            );
        }
    });

    Ok(())
}

/// Signals an in-progress `index_snapshots_batch` run to stop after the
/// currently-indexing snapshot finishes. Has no effect if no batch is running.
#[tauri::command]
pub fn cancel_index_batch(index_handle: State<'_, super::cache::IndexHandle>) -> Result<(), String> {
    index_handle
        .cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Runs on a blocking-pool thread (via `spawn_blocking`) rather than inline: this query
/// can take a second or more against a large index, and running it synchronously on an
/// async worker thread would hold the shared `AppDb` mutex there, starving unrelated
/// commands (snapshot list refreshes, status polling, the cache warmer tick) that also
/// need that mutex and would otherwise queue behind it.
#[tauri::command]
pub async fn search_snapshot_files(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    repo_id: String,
    snapshot_id: String,
    query: String,
) -> Result<Vec<FileEntry>, String> {
    validate_snapshot_id(&snapshot_id)?;
    let trimmed = query.trim().to_owned();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    let status_map = db.get_browse_status(&repo_id)?;
    match status_map.get(&snapshot_id).map(|s| s.as_str()) {
        Some("complete") => {}
        Some("in_progress") => {
            return Err("Snapshot is currently being indexed — try again shortly.".to_string())
        }
        _ => {
            return Err(
                "Snapshot is not indexed. Index it first to enable search.".to_string(),
            )
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        db.search_browse_files(&snapshot_id, &trimmed, 200)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Searches all fully-indexed snapshots of a repo at once. Each matching path
/// is returned once, attributed to the newest snapshot containing it (the
/// dedup + "pick newest" logic lives in the SQL, see `AppDb::search_repo_files`).
/// See `search_snapshot_files` above for why this runs via `spawn_blocking`.
#[tauri::command]
pub async fn search_repo_files(
    app: tauri::AppHandle,
    repo_id: String,
    query: String,
) -> Result<Vec<RepoFileHit>, String> {
    let trimmed = query.trim().to_owned();
    if trimmed.is_empty() {
        return Ok(vec![]);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        db.search_repo_files(&repo_id, &trimmed, 200)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Returns a map of snapshot_id → index status for all snapshots in a repo.
/// The frontend uses this to grey out "Index Snapshot" for already-indexed rows.
#[tauri::command]
pub fn get_snapshot_index_status(
    db: State<'_, AppDb>,
    repo_id: String,
) -> Result<HashMap<String, String>, String> {
    db.get_browse_status(&repo_id)
}

/// Removes the browse cache (files + status) for a single snapshot.
/// The frontend uses this to let the user clear an index without wiping all caches.
#[tauri::command]
pub fn clear_snapshot_index(
    db: State<'_, AppDb>,
    repo_id: String,
    snapshot_id: String,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;
    db.evict(&repo_id, &snapshot_id)
}

/// Aggregate "N of M snapshots indexed" across all eligible repos (respects
/// `remote_auto_refresh`, same filtering as the cache warmer's sweep). The Activity
/// panel uses this single call instead of fetching every repo's snapshot list plus
/// its index-status map and summing them on the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub cached: u64,
    pub total: u64,
}

#[tauri::command]
pub async fn get_index_progress(app: tauri::AppHandle) -> Result<IndexProgress, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        let remote_auto_refresh = db
            .get_setting("remote_auto_refresh", "false")
            .unwrap_or_else(|_| "false".to_string())
            == "true";

        let repos = db.list_repos()?;
        let eligible_repo_ids: Vec<String> = repos
            .into_iter()
            .filter(|r| remote_auto_refresh || !crate::cache_warmer::is_remote(&r.path))
            .map(|r| r.id)
            .collect();

        let (cached, total) = db.get_index_progress(&eligible_repo_ids)?;
        Ok(IndexProgress { cached, total })
    })
    .await
    .map_err(|e| e.to_string())?
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_direct_child_root_level() {
        // Root level (parent is None or "" or "/")
        // For 2-segment paths like "foo/bar", the second segment is the direct child
        assert!(is_direct_child("foo/bar", None));
        assert!(is_direct_child("foo/bar", Some("")));
        assert!(is_direct_child("foo/bar", Some("/")));
        
        // Single-segment paths have no "child" at root
        assert!(!is_direct_child("foo", None));
        
        // 3+ segments are not direct children
        assert!(!is_direct_child("foo/bar/baz", None));
    }

    #[test]
    fn test_is_direct_child_with_parent() {
        // With explicit parent, check if entry is a direct child
        assert!(is_direct_child("parent/child", Some("parent")));
        assert!(is_direct_child("parent/child", Some("parent/")));
        
        // Nested child should not be direct
        assert!(!is_direct_child("parent/child/grandchild", Some("parent")));
        
        // Wrong parent should not match
        assert!(!is_direct_child("other/child", Some("parent")));
        
        // Root as parent with 2-segment path
        assert!(is_direct_child("foo/bar", Some("/")));
    }

    #[test]
    fn test_is_direct_child_edge_cases() {
        // Trailing slashes are handled by trim_end_matches
        assert!(is_direct_child("parent/child", Some("parent")));
        assert!(is_direct_child("parent/child/", Some("parent")));
        
        // Two-level nesting with parent
        assert!(is_direct_child("a/b/c", Some("a/b")));
        assert!(!is_direct_child("a/b/c", Some("a")));
        
        // Empty path
        assert!(!is_direct_child("", None));
        
        // Paths with multiple segments
        assert!(is_direct_child("parent/child", Some("parent")));
    }

}
