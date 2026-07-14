use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, BackupHandle, CopyHandle, FullRepository, MasterKey, MirrorEntry, MirrorHandle, RetentionPolicy};
use super::repo::{run_restic_blocking, run_restic_with_path};
use super::repo_locks::RepoLocks;
use super::NoConsole;
use crate::tasks::{emit_cancelling, new_operation_id, new_task_slot, OperationCtx, TaskKind, TaskOrigin, TaskProgress};

/// Sentinel `backup_history.error` value for a genuinely cancelled backup, distinguishing it
/// from a real failure so Recent Logs / LogsPage can render it neutrally instead of as an
/// error (see the frontend's matching CANCELLED_BACKUP_ERROR in lib/types.ts).
pub(crate) const CANCELLED_BACKUP_ERROR: &str = "Cancelled";

/// Sentinel error string returned by `mirror_repo` when the exact same `(src, dest)` repo
/// pair already has a queued or running mirror — matches the frontend's
/// `MIRROR_ALREADY_ACTIVE_ERROR` (lib/types.ts) exactly, same pattern as
/// `INDEX_BATCH_ALREADY_ACTIVE_ERROR` (browse.rs). A different source or a different
/// destination is not a duplicate and queues normally — see `mirror_repo`'s doc comment.
pub(crate) const MIRROR_ALREADY_ACTIVE_ERROR: &str = "MirrorAlreadyActive";

/// Deregisters a mirror run's entry from `MirrorHandle::mirrors` on every exit path, mirroring
/// `BatchDeregisterGuard` (browse.rs) — so `cancel_mirror` can never target a run that has
/// already finished, and so a finished run's `(src, dest)` pair stops blocking a fresh mirror
/// between the same two repos.
struct MirrorDeregisterGuard {
    registry: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, MirrorEntry>>>,
    operation_id: String,
}
impl Drop for MirrorDeregisterGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.lock() {
            map.remove(&self.operation_id);
        }
    }
}

/// Testable core of `mirror_repo`'s `(src, dest)` duplicate-request guard, split out as a pure
/// predicate so it can be exercised in a unit test without constructing a `tauri::State` —
/// mirrors browse.rs's `batch_matches_repo` pattern. `mirror_repo` itself checks this under the
/// same lock acquisition it registers the new entry under (see its body), so the check and the
/// registration are atomic; this function only expresses the per-entry comparison.
fn mirror_pair_matches(entry: &MirrorEntry, src_id: &str, dest_id: &str) -> bool {
    entry.src_id == src_id && entry.dest_id == dest_id
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub repo_id: String,
    pub plan_id: Option<String>,
    pub percent_done: f64,
    pub files_done: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub seconds_elapsed: u64,
    pub seconds_remaining: Option<u64>,
    pub current_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    pub hostname: String,
    pub username: Option<String>,
    pub paths: Vec<String>,
    pub tags: Option<Vec<String>>,
}

#[tauri::command]
pub async fn list_snapshots(
    db: State<'_, AppDb>,
    repo_id: String,
) -> Result<Vec<Snapshot>, String> {
    db.get_snapshots_vec(&repo_id)
}

#[tauri::command]
pub async fn refresh_snapshots(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<Vec<Snapshot>, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let _rg = repo_locks.read(&repo.path);
    let stdout = run_restic_blocking(repo, vec!["snapshots".into(), "--json".into()], restic_path).await?;
    let _ = db.set_snapshots(&repo_id, &stdout);
    let snapshots: Vec<Snapshot> = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
    Ok(snapshots)
}

/// Runs `run_restic_blocking`, retrying up to twice (2s apart) if the failure is restic's own
/// "already locked" error — a residual collision with a *different* machine's or tool's genuine
/// repo lock that `RepoLocks` (this process's own in-memory coordination — see CLAUDE.md's
/// Concurrency section) has no visibility into. Matches `apply_retention`'s retry pattern
/// exactly, just async (`tokio::time::sleep`) instead of sync (`std::thread::sleep`) since every
/// caller here is an async `#[tauri::command]`. Any other error surfaces immediately — only a
/// lock collision is worth retrying blind.
async fn run_restic_blocking_retrying_on_lock(
    repo: FullRepository,
    args: Vec<String>,
    restic_path: String,
) -> Result<String, String> {
    let mut result = run_restic_blocking(repo.clone(), args.clone(), restic_path.clone()).await;
    for _ in 0..2 {
        match &result {
            Err(e) if e.contains("already locked") => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                result = run_restic_blocking(repo.clone(), args.clone(), restic_path.clone()).await;
            }
            _ => break,
        }
    }
    result
}

#[tauri::command]
pub async fn delete_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    prune: bool,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Forget,
        repo_id.clone(),
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        None,
    );
    let mut args = vec!["forget".to_string(), snapshot_id.clone()];
    if prune {
        args.push("--prune".to_string());
    }
    // `forget` (with or without --prune) takes restic's exclusive lock — wait for the
    // repo to go idle first (see CLAUDE.md's Concurrency section / repo_locks.rs).
    let _wg = repo_locks.write(&repo.path).await;
    let result = run_restic_blocking_retrying_on_lock(repo, args, restic_path).await;
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result?;
    let _ = db.evict(&repo_id, &snapshot_id);  // clears browse_cache_files + browse_cache_status
    let _ = db.evict_snapshots(&repo_id);
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn tag_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Tag,
        repo_id,
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        None,
    );
    // `tag` modifies snapshot metadata — exclusive lock, same as forget/prune.
    let _wg = repo_locks.write(&repo.path).await;

    let result: Result<(), String> = async {
        if !add_tags.is_empty() {
            let tag_str = add_tags.join(",");
            run_restic_blocking_retrying_on_lock(
                repo.clone(),
                vec!["tag".into(), "--add".into(), tag_str, snapshot_id.clone()],
                restic_path.clone(),
            )
            .await?;
        }
        if !remove_tags.is_empty() {
            let tag_str = remove_tags.join(",");
            run_restic_blocking_retrying_on_lock(
                repo,
                vec!["tag".into(), "--remove".into(), tag_str, snapshot_id.clone()],
                restic_path,
            )
            .await?;
        }
        Ok(())
    }
    .await;

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

pub(crate) fn validate_snapshot_id(id: &str) -> Result<(), String> {
    if id.len() < 8 || id.len() > 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid snapshot ID: '{id}'"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotStats {
    pub total_size: u64,
    pub total_file_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffEntry {
    pub path: String,
    pub change: String, // "added" | "removed" | "modified"
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub entries: Vec<DiffEntry>,
    pub total_added: u32,
    pub total_removed: u32,
    pub total_modified: u32,
    pub truncated: bool,
}

const DIFF_ENTRY_LIMIT: usize = 500;

/// Parses `restic diff`'s plain-text output (`+`/`-`/`M`/`T`-prefixed lines) into a
/// `DiffResult`, capping stored entries at `DIFF_ENTRY_LIMIT` while still counting the
/// full totals (so `truncated` reflects reality). Pure — no restic call — so
/// `diff_snapshots` (the async command wrapper) can be tested by feeding it captured
/// stdout instead of shelling out to a real restic binary.
fn parse_diff_output(stdout: &str) -> DiffResult {
    let mut entries: Vec<DiffEntry> = Vec::new();
    let mut total_added = 0u32;
    let mut total_removed = 0u32;
    let mut total_modified = 0u32;

    for line in stdout.lines() {
        let (change, path) = if let Some(path) = line.strip_prefix("+  ") {
            total_added += 1;
            ("added", path)
        } else if let Some(path) = line.strip_prefix("-  ") {
            total_removed += 1;
            ("removed", path)
        } else if let Some(path) = line.strip_prefix("M  ").or_else(|| line.strip_prefix("T  ")) {
            total_modified += 1;
            ("modified", path)
        } else {
            continue;
        };

        if entries.len() < DIFF_ENTRY_LIMIT {
            entries.push(DiffEntry { path: path.trim().to_string(), change: change.to_string() });
        }
    }

    let total = total_added + total_removed + total_modified;
    let truncated = total as usize > entries.len();

    DiffResult { entries, total_added, total_removed, total_modified, truncated }
}

#[tauri::command]
pub async fn diff_snapshots(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_a: String,
    snapshot_b: String,
) -> Result<DiffResult, String> {
    validate_snapshot_id(&snapshot_a)?;
    validate_snapshot_id(&snapshot_b)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Diff,
        repo_id,
        Some(format!("{snapshot_a}..{snapshot_b}")),
        TaskOrigin::Manual,
        None,
    );
    let _rg = repo_locks.read(&repo.path);
    let result = run_restic_blocking(
        repo,
        vec!["diff".into(), snapshot_a.clone(), snapshot_b.clone()],
        restic_path,
    )
    .await;

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    let stdout = result?;

    Ok(parse_diff_output(&stdout))
}

#[tauri::command]
pub async fn get_snapshot_stats(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
) -> Result<SnapshotStats, String> {
    validate_snapshot_id(&snapshot_id)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Stats,
        repo_id,
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        None,
    );
    let _rg = repo_locks.read(&repo.path);
    let result = run_restic_blocking(
        repo,
        vec!["stats".into(), "--json".into(), snapshot_id.clone()],
        restic_path,
    )
    .await;
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    let stdout = result?;
    let last_line = super::repo::last_nonblank_line(&stdout)
        .ok_or_else(|| "No output from restic stats".to_string())?;
    let v: serde_json::Value = serde_json::from_str(last_line).map_err(|e| e.to_string())?;
    Ok(SnapshotStats {
        total_size: v["total_size"].as_u64().unwrap_or(0),
        total_file_count: v["total_file_count"].as_u64().unwrap_or(0),
    })
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_backup(
    app: &tauri::AppHandle,
    db: &AppDb,
    master_key: &MasterKey,
    backup_handle: &BackupHandle,
    repo_locks: &RepoLocks,
    repo_id: &str,
    plan_id: Option<&str>,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
    limit_upload: Option<u32>,
    limit_download: Option<u32>,
    origin: TaskOrigin,
) -> Result<String, String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Serialize backups: only one may run at a time. A second concurrent attempt
    // (e.g. a scheduler tick firing while a manual backup runs) would otherwise
    // overwrite the shared `child`/`cancelled` state on the BackupHandle.
    if backup_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A backup is already in progress".to_string());
    }
    // Released on every exit path — including `?` early returns, panics, and
    // future cancellation — so `busy` never gets stuck set.
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&backup_handle.busy);

    let key = master_key.get()?;
    let repo = db.get_full_repo(repo_id, &key)?;
    let restic_path = super::get_restic_path(db);
    let compression = db.get_setting("compression", "auto").unwrap_or_else(|_| "auto".to_string());

    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Backup,
        repo_id,
        plan_id.map(str::to_string),
        origin,
        Some(backup_handle.current_task.clone()),
    );
    let task_progress = task_ctx.progress_emitter();

    let mut args: Vec<String> = vec!["backup".to_string(), "--json".to_string()];
    for tag in &tags {
        args.push("--tag".to_string());
        args.push(tag.clone());
    }
    for pattern in &excludes {
        let trimmed = pattern.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            args.push("--exclude".to_string());
            args.push(trimmed.to_string());
        }
    }
    if let Some(kib) = limit_upload.filter(|&v| v > 0) {
        args.push("--limit-upload".to_string());
        args.push(kib.to_string());
    }
    if let Some(kib) = limit_download.filter(|&v| v > 0) {
        args.push("--limit-download".to_string());
        args.push(kib.to_string());
    }
    for path in &paths {
        args.push(path.clone());
    }

    // Touch each path so macOS TCC prompts appear upfront, attributed to
    // "Resty Desktop", before restic is spawned. Child processes inherit the
    // grants so restic won't re-trigger them. Not needed on other platforms.
    #[cfg(target_os = "macos")]
    for path in &paths {
        let _ = std::fs::metadata(path);
    }

    let started = std::time::Instant::now();
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let app_inner = app.clone();
    let repo_path = repo.path.clone();
    let repo_password = repo.password.clone();
    let repo_path_for_unlock = repo.path.clone();
    let repo_pass_for_unlock = repo.password.clone();
    let restic_path_inner = restic_path.clone();
    let restic_path_for_unlock = restic_path.clone();
    let compression_inner = compression.clone();
    let repo_id_inner = repo_id.to_string();
    let plan_id_inner = plan_id.map(|s| s.to_string());
    let task_progress_inner = task_progress.clone();

    // `backup` takes restic's shared lock — register as a reader so an exclusive
    // op (forget/prune/tag) on this repo knows to wait for us. Held across the
    // spawn_blocking below so it stays claimed for the whole child-process
    // lifetime, not just this setup.
    let _rg = repo_locks.read(&repo.path);

    backup_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&backup_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&backup_handle.cancelled);

    let result: Result<String, String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut cmd = std::process::Command::new(&restic_path_inner);
        cmd.args(&args)
            .env("RESTIC_REPOSITORY", &repo_path)
            .env("RESTIC_COMPRESSION", &compression_inner);
        super::repo::apply_repo_password(&mut cmd, &repo_password);
        let mut child = cmd
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

        // Store child so cancel_backup can reach it.
        *child_arc.lock().map_err(|e| e.to_string())? = Some(child);

        let reader = BufReader::new(stdout);
        let mut summary_line: Option<String> = None;

        // Drain to EOF unconditionally, even after a cancel is requested — the killed
        // process closes stdout quickly on its own, and breaking early here risks missing
        // the trailing `summary` line if the process happens to finish successfully in the
        // same instant Stop is clicked (the reordered check below then needs summary_line
        // to be complete for the success case, exactly like copy_snapshot/mirror_repo drain
        // unconditionally).
        for line in reader.lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                match v["message_type"].as_str() {
                    Some("status") => {
                        let progress = BackupProgress {
                            repo_id: repo_id_inner.clone(),
                            plan_id: plan_id_inner.clone(),
                            percent_done: v["percent_done"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0),
                            files_done: v["files_done"].as_u64().unwrap_or(0),
                            total_files: v["total_files"].as_u64().unwrap_or(0),
                            bytes_done: v["bytes_done"].as_u64().unwrap_or(0),
                            total_bytes: v["total_bytes"].as_u64().unwrap_or(0),
                            seconds_elapsed: v["seconds_elapsed"].as_u64().unwrap_or(0),
                            seconds_remaining: v["seconds_remaining"].as_u64(),
                            current_files: v["current_files"]
                                .as_array()
                                .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
                                .unwrap_or_default(),
                        };
                        let _ = app_inner.emit("backup:progress", &progress);
                        task_progress_inner.emit(TaskProgress {
                            percent_done: Some(progress.percent_done),
                            items_done: Some(progress.files_done),
                            items_total: Some(progress.total_files),
                            bytes_done: Some(progress.bytes_done),
                            bytes_total: Some(progress.total_bytes),
                            label: None,
                            seconds_elapsed: Some(progress.seconds_elapsed),
                            seconds_remaining: progress.seconds_remaining,
                            current_files: Some(progress.current_files.clone()),
                            repo_id: None,
                        });
                    }
                    Some("summary") => summary_line = Some(line),
                    _ => {}
                }
            }
        }

        let status = match child_arc.lock().map_err(|e| e.to_string())?.take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };
        let stderr_str = stderr_thread.join().unwrap_or_default();

        // A successful exit always wins, even if `cancelled` got set in a race (e.g. Stop
        // clicked just as restic finished) — the backup genuinely completed and must not
        // be reported as cancelled (which would also discard a valid summary_line).
        if status.success() {
            return Ok(summary_line.unwrap_or_default());
        }

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }

        let msg = stderr_str.trim();
        let base = if msg.is_empty() { "restic backup failed".to_string() } else { msg.to_string() };
        #[cfg(target_os = "macos")]
        if base.to_lowercase().contains("permission denied") || base.to_lowercase().contains("operation not permitted") {
            return Err(format!("{base}\n\nSome paths require Full Disk Access. Go to System Settings → Privacy & Security → Full Disk Access and add Resty Desktop."));
        }
        Err(base)
    })
    .await
    .map_err(|e| e.to_string())?;

    // Mirrors the "successful exit always wins over a raced cancel" rule just
    // below: report `Finished` even if `cancelled` got set in the same race.
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if backup_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }

    // Only unlock on a genuine cancel-kill: since a raced success now wins over the
    // cancelled flag (see the reordering above), `cancelled` can be true even when
    // `result` is `Ok` — that repo was never left locked, so skip the needless unlock.
    if result.is_err() && backup_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        // Deliberately fire-and-forget: unlock is a best-effort cleanup, not something
        // the caller needs to await (see the "restic unlock calls are exempt" note in
        // CLAUDE.md's Concurrency section).
        #[allow(clippy::let_underscore_future)]
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let repo = FullRepository { path: repo_path_for_unlock, password: repo_pass_for_unlock };
            let _ = run_restic_with_path(&repo, vec!["unlock"], &restic_path_for_unlock);
        });
    }

    let duration = started.elapsed().as_secs_f64();

    use tauri_plugin_notification::NotificationExt;
    use rand::Rng;
    let history_id: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();

    match result {
        Ok(ref stdout) => {
            // Stats cache is intentionally left alone here — it only refreshes via the
            // manual Refresh button/Refresh All now (see repo.rs's refresh_repo_stats).
            let summary = stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|v| v.get("message_type").and_then(|t| t.as_str()) == Some("summary"));

            let snapshot_id = summary.as_ref()
                .and_then(|v| v["snapshot_id"].as_str().map(str::to_string));
            let files_new = summary.as_ref().and_then(|v| v["files_new"].as_u64()).unwrap_or(0);
            let files_changed = summary.as_ref().and_then(|v| v["files_changed"].as_u64()).unwrap_or(0);
            let bytes_added = summary.as_ref().and_then(|v| v["data_added"].as_u64()).unwrap_or(0);

            if let Some(ref id) = snapshot_id {
                if let Ok(new_json) = run_restic_with_path(&repo, vec!["snapshots", "--json", id], &restic_path) {
                    let _ = db.append_snapshots(repo_id, &new_json);
                } else {
                    let _ = db.evict_snapshots(repo_id);
                }
            }

            let _ = db.log_backup(
                &history_id, repo_id, plan_id, snapshot_id.as_deref(),
                started_at, duration, files_new, files_changed, bytes_added, None,
            );
            // Fires for every backup (manual or scheduled) — the Activity panel's Recent
            // Logs section listens for this to refresh; the scheduler-only `task` bus
            // lifecycle (origin: "scheduler", see activity.tsx's reduceSchedulerBackup)
            // doesn't cover manual runs, so this stays the one signal both share.
            let _ = app.emit("backup:history-updated", ());

            let body = format!(
                "{} new, {} changed · {:.1}s",
                files_new, files_changed, duration
            );
            let _ = app.notification().builder()
                .title("Backup completed")
                .body(&body)
                .show();

            Ok(stdout.clone())
        }
        Err(ref err) => {
            // A genuine cancellation is not a failure — log/notify it distinctly instead of
            // the raw internal "cancelled" control-flow string, which would otherwise always
            // log/notify as "Backup failed" regardless of was_cancelled.
            let was_cancelled = backup_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst);
            let log_error = if was_cancelled { CANCELLED_BACKUP_ERROR.to_string() } else { err.clone() };
            let _ = db.log_backup(
                &history_id, repo_id, plan_id, None,
                started_at, duration, 0, 0, 0, Some(log_error.as_str()),
            );
            let _ = app.emit("backup:history-updated", ());

            if was_cancelled {
                let _ = app.notification().builder()
                    .title("Backup cancelled")
                    .body("The backup was cancelled before it finished.")
                    .show();
            } else {
                let _ = app.notification().builder()
                    .title("Backup failed")
                    .body(err)
                    .show();
            }

            Err(err.clone())
        }
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn run_backup(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    backup_handle: State<'_, BackupHandle>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    plan_id: Option<String>,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
    limit_upload: Option<u32>,
    limit_download: Option<u32>,
) -> Result<String, String> {
    execute_backup(&app, &db, &master_key, &backup_handle, &repo_locks, &repo_id, plan_id.as_deref(), paths, tags, excludes, limit_upload, limit_download, TaskOrigin::Manual).await
}

#[tauri::command]
pub async fn cancel_backup(app: tauri::AppHandle, backup_handle: State<'_, BackupHandle>) -> Result<(), String> {
    emit_cancelling(&app, &backup_handle.current_task);
    backup_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut guard) = backup_handle.child.lock() {
        if let Some(ref mut child) = *guard {
            let _ = child.kill();
        }
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn copy_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    copy_handle: State<'_, CopyHandle>,
    repo_locks: State<'_, RepoLocks>,
    src_repo_id: String,
    dest_repo_id: String,
    snapshot_id: String,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;

    use std::sync::atomic::{AtomicBool, Ordering};

    // Serialize copies: only one may run at a time — this handle previously had no busy
    // guard, so a second concurrent copy could clobber the first run's child/cancelled state.
    if copy_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A copy is already in progress".to_string());
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&copy_handle.busy);

    let key = master_key.get()?;
    let src_repo = db.get_full_repo(&src_repo_id, &key)?;
    let dest_repo = db.get_full_repo(&dest_repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    // `repoId` is the destination (where data lands / cache is evicted below);
    // the source repo isn't separately representable in the single-repoId envelope.
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Copy,
        dest_repo_id.clone(),
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        Some(copy_handle.current_task.clone()),
    );

    copy_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&copy_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&copy_handle.cancelled);

    // Stash credentials so we can run `restic unlock` after a cancel-kill.
    let src_path_for_unlock = src_repo.path.clone();
    let src_pass_for_unlock = src_repo.password.clone();
    let dst_path_for_unlock = dest_repo.path.clone();
    let dst_pass_for_unlock = dest_repo.password.clone();
    let restic_path_for_unlock = restic_path.clone();

    // `copy` reads from src and writes new blobs into dest, both under restic's shared
    // lock — register as a reader on both so an exclusive op (forget/prune/tag) on
    // either repo knows to wait for us. Held across the spawn_blocking below.
    let _src_rg = repo_locks.read(&src_repo.path);
    let _dst_rg = repo_locks.read(&dest_repo.path);

    let result: Result<(), String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut cmd = std::process::Command::new(&restic_path);
        cmd.args(["copy", &snapshot_id])
            .env("RESTIC_REPOSITORY", &dest_repo.path)
            .env("RESTIC_FROM_REPOSITORY", &src_repo.path);
        super::repo::apply_repo_password(&mut cmd, &dest_repo.password);
        super::repo::apply_from_repo_password(&mut cmd, &src_repo.password);
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .no_console()
            .augment_path()
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        let stderr = child.stderr.take().ok_or("failed to capture restic stderr")?;
        let stdout = child.stdout.take().ok_or("failed to capture restic stdout")?;

        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        // Store child so cancel_copy can reach it.
        *child_arc.lock().map_err(|e| e.to_string())? = Some(child);

        // Drain stdout so the process isn't blocked on a full pipe buffer.
        for _ in BufReader::new(stdout).lines() {}

        // stdout exhausted (process ended or was killed); take child back to call wait().
        let status = match child_arc.lock().map_err(|e| e.to_string())?.take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };

        let stderr_str = stderr_thread.join().unwrap_or_default();

        // A successful exit always wins, even if `cancelled` got set in a race (e.g. Stop
        // clicked just as restic finished) — the copy genuinely completed and must not be
        // reported as cancelled.
        if status.success() {
            return Ok(());
        }

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }

        let msg = stderr_str.trim();
        Err(if msg.is_empty() { "restic copy failed".to_string() } else { msg.to_string() })
    })
    .await
    .map_err(|e| e.to_string())?;

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if copy_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }

    if result.is_ok() {
        let _ = db.evict_snapshots(&dest_repo_id);
    } else if copy_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        // The process was killed via SIGKILL and left stale locks on both repos.
        // spawn_blocking already called wait(), so the PIDs are gone — unlock is safe now.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let src = FullRepository { path: src_path_for_unlock, password: src_pass_for_unlock };
            let dst = FullRepository { path: dst_path_for_unlock, password: dst_pass_for_unlock };
            let _ = run_restic_with_path(&src, vec!["unlock"], &restic_path_for_unlock);
            let _ = run_restic_with_path(&dst, vec!["unlock"], &restic_path_for_unlock);
        })
        .await;
    }
    result
}

#[tauri::command]
pub async fn cancel_copy(app: tauri::AppHandle, copy_handle: State<'_, CopyHandle>) -> Result<(), String> {
    emit_cancelling(&app, &copy_handle.current_task);
    copy_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(ref mut child) = *copy_handle.child.lock().map_err(|e| e.to_string())? {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Queues a mirror run and returns immediately with its `operationId` — the run itself
/// (waiting its turn, then copying) happens in a detached `spawn`ed task, the same
/// fire-and-forget shape as `index_snapshots_batch` (browse.rs). The frontend uses the
/// returned id to correlate every subsequent `task` event and to target `cancel_mirror`
/// precisely, rather than by `repoId` — the envelope's `repoId` is the *destination*, and
/// this command deliberately allows multiple mirrors into the same destination from
/// different sources (or the same source into different destinations) to be queued at
/// once, so `repoId` alone can't tell two such runs apart (see this module's design notes
/// in CLAUDE.md's Operation Event Bus section).
///
/// Only an exact `(src_repo_id, dest_repo_id)` repeat — one already queued or running — is
/// rejected with `MIRROR_ALREADY_ACTIVE_ERROR`; a different source or a different
/// destination queues normally. The check and the registration happen under one
/// `mirrors` lock acquisition, so there's no window between "checked" and "registered"
/// where two identical requests could both slip through (tighter than
/// `index_snapshots_batch`'s analogous check, which tolerates that narrow race as
/// acceptable for human-paced clicks).
#[tauri::command]
pub async fn mirror_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    mirror_handle: State<'_, MirrorHandle>,
    src_repo_id: String,
    dest_repo_id: String,
) -> Result<String, String> {
    use std::sync::atomic::Ordering;

    let key = master_key.get()?;
    let src_repo = db.get_full_repo(&src_repo_id, &key)?;
    let dest_repo = db.get_full_repo(&dest_repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let operation_id = new_operation_id();
    let entry = MirrorEntry {
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        task_slot: new_task_slot(),
        cancel_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
        started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        child: std::sync::Arc::new(std::sync::Mutex::new(None)),
        src_id: src_repo_id.clone(),
        dest_id: dest_repo_id.clone(),
    };

    {
        let mut map = mirror_handle.mirrors.lock().map_err(|e| e.to_string())?;
        if map.values().any(|e| mirror_pair_matches(e, &src_repo_id, &dest_repo_id)) {
            return Err(MIRROR_ALREADY_ACTIVE_ERROR.to_string());
        }
        map.insert(operation_id.clone(), entry.clone());
    }

    // Stash credentials so the task can run `restic unlock` after a cancel-kill, and so it
    // doesn't need to re-resolve the master key (which may since have locked) or re-fetch
    // from `db` after the outer command has already returned.
    let src_path_for_unlock = src_repo.path.clone();
    let src_pass_for_unlock = src_repo.password.clone();
    let dst_path_for_unlock = dest_repo.path.clone();
    let dst_pass_for_unlock = dest_repo.password.clone();
    let restic_path_for_unlock = restic_path.clone();

    let registry = std::sync::Arc::clone(&mirror_handle.mirrors);
    let turn = std::sync::Arc::clone(&mirror_handle.turn);
    let op_id_for_task = operation_id.clone();
    let app_for_task = app.clone();

    tauri::async_runtime::spawn(async move {
        // Removes this run's entry from `mirrors` on every exit path below (including the
        // early `return` in the cancelled-while-queued branch), mirroring
        // `BatchDeregisterGuard` — see its doc comment.
        let _dereg = MirrorDeregisterGuard { registry, operation_id: op_id_for_task.clone() };

        let task_ctx = OperationCtx::new_pending_with_id(
            app_for_task.clone(),
            TaskKind::Mirror,
            dest_repo_id.clone(),
            None,
            TaskOrigin::Manual,
            Some(entry.task_slot.clone()),
            op_id_for_task,
        );

        // Wait for this run's turn so only one mirror ever has a live child at once (see
        // MirrorHandle::turn's doc comment). Cancellable while queued: a run parked here
        // (already registered above, so its Stop button works) bails the moment
        // cancel_mirror fires cancel_notify, instead of waiting for the run ahead of it to
        // finish. `biased` + `notify_one`'s stored-permit semantics make this race-free even
        // if cancel arrives before this select! starts polling — same reasoning as
        // index_snapshots_batch's identical pattern (browse.rs).
        let _turn = tokio::select! {
            biased;
            _ = entry.cancel_notify.notified() => {
                task_ctx.cancelled();
                return;
            }
            permit = turn.lock() => permit,
        };
        // Won the turn — this run is now actually executing. Promotes it from the Activity
        // panel's "Up Next" area into Active Tasks.
        task_ctx.activate();
        entry.started.store(true, Ordering::SeqCst);

        // Acquired here (inside the task), not in the outer command — the command already
        // returned once this run was queued, so a guard taken in the outer scope would have
        // dropped before the copy ever ran. `ReadGuard` owns an `Arc` internally (see
        // repo_locks.rs), so it's safe to hold across this whole async block.
        let repo_locks = app_for_task.state::<RepoLocks>();
        let _src_rg = repo_locks.read(&src_repo.path);
        let _dst_rg = repo_locks.read(&dest_repo.path);

        let child_arc = std::sync::Arc::clone(&entry.child);
        let cancel_arc = std::sync::Arc::clone(&entry.cancel);
        let src_repo_inner = src_repo.clone();
        let dest_repo_inner = dest_repo.clone();
        let restic_path_inner = restic_path.clone();

        let result: Result<(), String> = tauri::async_runtime::spawn_blocking(move || {
            use std::io::{BufRead, BufReader, Read};
            use std::process::Stdio;

            let mut cmd = std::process::Command::new(&restic_path_inner);
            cmd.args(["copy"])
                .env("RESTIC_REPOSITORY", &dest_repo_inner.path)
                .env("RESTIC_FROM_REPOSITORY", &src_repo_inner.path);
            super::repo::apply_repo_password(&mut cmd, &dest_repo_inner.password);
            super::repo::apply_from_repo_password(&mut cmd, &src_repo_inner.password);
            let mut child = cmd
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .no_console()
                .augment_path()
                .spawn()
                .map_err(|e| format!("Failed to run restic: {e}"))?;

            let stderr = child.stderr.take().ok_or("failed to capture restic stderr")?;
            let stdout = child.stdout.take().ok_or("failed to capture restic stdout")?;

            let stderr_thread = std::thread::spawn(move || {
                let mut s = String::new();
                BufReader::new(stderr).read_to_string(&mut s).ok();
                s
            });

            *child_arc.lock().map_err(|e| e.to_string())? = Some(child);

            // Drain stdout to avoid blocking the process on a full pipe buffer.
            for _ in BufReader::new(stdout).lines() {}

            let status = match child_arc.lock().map_err(|e| e.to_string())?.take() {
                Some(mut c) => c.wait().map_err(|e| e.to_string())?,
                None => return Err("cancelled".to_string()),
            };

            let stderr_str = stderr_thread.join().unwrap_or_default();

            // A successful exit always wins, even if `cancel` got set in a race (e.g. Stop
            // clicked just as restic finished) — the mirror genuinely completed and must
            // not be reported as cancelled.
            if status.success() {
                return Ok(());
            }

            if cancel_arc.load(Ordering::SeqCst) {
                return Err("cancelled".to_string());
            }

            let msg = stderr_str.trim();
            Err(if msg.is_empty() { "restic mirror failed".to_string() } else { msg.to_string() })
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);

        let cancelled = entry.cancel.load(Ordering::SeqCst);
        match &result {
            Ok(_) => task_ctx.finished(),
            Err(_) if cancelled => task_ctx.cancelled(),
            Err(e) => task_ctx.failed(e.clone()),
        }

        if result.is_ok() {
            let _ = app_for_task.state::<AppDb>().evict_snapshots(&dest_repo_id);
        } else if cancelled {
            // The process was killed via SIGKILL and left stale locks on both repos.
            // spawn_blocking already called wait(), so the PIDs are gone — unlock is safe now.
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let src = FullRepository { path: src_path_for_unlock, password: src_pass_for_unlock };
                let dst = FullRepository { path: dst_path_for_unlock, password: dst_pass_for_unlock };
                let _ = run_restic_with_path(&src, vec!["unlock"], &restic_path_for_unlock);
                let _ = run_restic_with_path(&dst, vec!["unlock"], &restic_path_for_unlock);
            })
            .await;
        }
    });

    Ok(operation_id)
}

/// Signals a specific queued or running mirror (identified by its operationId — see
/// `mirror_repo`'s doc comment) to stop. A no-op, not an error, if that id doesn't match any
/// tracked run (already finished, or never existed) — mirrors `cancel_index_batch`'s
/// (browse.rs) same-shaped no-op behavior, now scoped per-run instead of app-wide.
#[tauri::command]
pub fn cancel_mirror(
    app: tauri::AppHandle,
    mirror_handle: State<'_, MirrorHandle>,
    operation_id: String,
) -> Result<(), String> {
    let entry = mirror_handle
        .mirrors
        .lock()
        .map_err(|e| e.to_string())?
        .get(&operation_id)
        .cloned();
    if let Some(entry) = entry {
        emit_cancelling(&app, &entry.task_slot);
        entry.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        // Wakes the run if it's currently parked waiting for its turn on
        // MirrorHandle::turn, so a queued mirror cancels immediately instead of waiting for
        // the run ahead of it to finish. A harmless no-op if the run has already won its
        // turn and is executing (in which case the kill below handles it) — see
        // MirrorEntry::cancel_notify's doc comment.
        entry.cancel_notify.notify_one();
        if let Some(ref mut child) = *entry.child.lock().map_err(|e| e.to_string())? {
            child.kill().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn unlock_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(app, TaskKind::Unlock, repo_id, None, TaskOrigin::Manual, None);
    let result = run_restic_blocking(repo, vec!["unlock".into()], restic_path).await;
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result?;
    Ok(())
}

fn build_retention_args(tags: &[String], paths: &[String], retention: &RetentionPolicy) -> Vec<String> {
    let mut args: Vec<String> =
        vec!["forget".to_string(), "--prune".to_string(), "--json".to_string()];

    if !tags.is_empty() {
        args.push("--group-by".to_string());
        args.push("tags".to_string());
        args.push("--tag".to_string());
        args.push(tags.join(","));
    } else {
        for path in paths {
            args.push("--path".to_string());
            args.push(path.clone());
        }
    }

    if let Some(n) = retention.keep_last {
        args.push("--keep-last".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_daily {
        args.push("--keep-daily".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_weekly {
        args.push("--keep-weekly".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_monthly {
        args.push("--keep-monthly".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_yearly {
        args.push("--keep-yearly".to_string());
        args.push(n.to_string());
    }

    args
}

#[allow(clippy::too_many_arguments)]
pub fn apply_retention(
    app: &tauri::AppHandle,
    db: &AppDb,
    master_key: &MasterKey,
    repo_locks: &RepoLocks,
    repo_id: &str,
    plan_id: Option<&str>,
    tags: &[String],
    paths: &[String],
    retention: &RetentionPolicy,
    // apply_retention always runs as the tail of a backup — the caller passes
    // through the same origin the triggering execute_backup call used (see
    // execute_backup's `origin` param).
    origin: TaskOrigin,
) -> Result<String, String> {
    // OperationCtx is created before the fallible key/repo lookups below (with each
    // explicitly calling task_ctx.failed() instead of relying on `?`), same pattern as
    // fetch_and_cache_stats (repo.rs) — this is a scheduled backup's *retention* step,
    // which the Activity panel's activeBackup row (activity.tsx's reduceSchedulerBackup)
    // is already displaying while waiting specifically for this op's "started" event. If a
    // lookup failed early via `?` and skipped OperationCtx entirely, that row would never
    // receive a task event and get stuck showing stale "backup finished" state until some
    // other scheduled backup happened to displace it.
    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Forget,
        repo_id,
        plan_id.map(str::to_string),
        origin,
        None,
    );

    let key = match master_key.get() {
        Ok(k) => k,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let repo = match db.get_full_repo(repo_id, &key) {
        Ok(r) => r,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let restic_path = super::get_restic_path(db);

    let args = build_retention_args(tags, paths, retention);
    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // `forget` takes restic's exclusive lock — wait for the repo to go idle first (see
    // CLAUDE.md's Concurrency section / repo_locks.rs). This is the primary fix for the
    // "repository is already locked" race (e.g. the user navigating to SnapshotsPage
    // right as this backup's retention kicks in, colliding with its background
    // `refreshSnapshots`); the retry below is now just defense-in-depth for anything not
    // routed through RepoLocks (an external restic/cron process).
    let _wg = repo_locks.write_blocking(&repo.path);
    let mut result = run_restic_with_path(&repo, args_refs.clone(), &restic_path);
    for _ in 0..2 {
        match &result {
            Err(e) if e.contains("already locked") => {
                std::thread::sleep(std::time::Duration::from_secs(2));
                result = run_restic_with_path(&repo, args_refs.clone(), &restic_path);
            }
            _ => break,
        }
    }

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }

    if result.is_ok() {
        if let Ok(json) =
            run_restic_with_path(&repo, vec!["snapshots", "--json"], &restic_path)
        {
            let _ = db.set_snapshots(repo_id, &json);
        } else {
            let _ = db.evict_snapshots(repo_id);
        }
    }
    result
}

/// Records a failed retention application as its own `backup_history` row so it's visible in
/// Recent Logs / LogsPage even though `apply_retention` has no history entry of its own.
/// Called by every retention call site (manual `forget_by_plan`, the 60s scheduler tick,
/// `run_schedule_now`) whenever `apply_retention` returns `Err` — otherwise all three would
/// silently discard that error, so a backup could succeed while its retention prune failed
/// with the failure visible nowhere.
pub(crate) fn log_retention_failure(
    app: &tauri::AppHandle,
    db: &AppDb,
    repo_id: &str,
    plan_id: Option<&str>,
    error: &str,
) {
    let _ = db.log_backup(
        &uuid::Uuid::new_v4().to_string(),
        repo_id,
        plan_id,
        None,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        0.0,
        0,
        0,
        0,
        Some(&format!("Retention failed: {error}")),
    );
    // Same event Recent Logs already listens for on every backup outcome — see
    // execute_backup's log_backup call sites above.
    let _ = app.emit("backup:history-updated", ());
}

#[tauri::command]
pub async fn forget_by_plan(
    app: tauri::AppHandle,
    repo_id: String,
    plan_id: Option<String>,
    tags: Vec<String>,
    paths: Vec<String>,
    retention: RetentionPolicy,
) -> Result<String, String> {
    let app_for_blocking = app.clone();
    let repo_id_for_blocking = repo_id.clone();
    let plan_id_for_blocking = plan_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let db = app_for_blocking.state::<AppDb>();
        let master_key = app_for_blocking.state::<MasterKey>();
        let repo_locks = app_for_blocking.state::<RepoLocks>();
        apply_retention(
            &app_for_blocking,
            db.inner(),
            master_key.inner(),
            repo_locks.inner(),
            &repo_id_for_blocking,
            plan_id_for_blocking.as_deref(),
            &tags,
            &paths,
            &retention,
            // forget_by_plan is the manual "Apply Retention" button (BackupPlansPage).
            TaskOrigin::Manual,
        )
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Err(ref e) = result {
        let db = app.state::<AppDb>();
        log_retention_failure(&app, db.inner(), &repo_id, plan_id.as_deref(), e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{build_retention_args, parse_diff_output, validate_snapshot_id};
    use crate::commands::cache::RetentionPolicy;

    #[test]
    fn accepts_8_char_hex_id() {
        assert!(validate_snapshot_id("abcdef01").is_ok());
    }

    #[test]
    fn accepts_64_char_hex_id() {
        let id = "a".repeat(64);
        assert!(validate_snapshot_id(&id).is_ok());
    }

    #[test]
    fn accepts_mixed_case_hex() {
        assert!(validate_snapshot_id("AbCdEf01").is_ok());
    }

    #[test]
    fn rejects_id_shorter_than_8_chars() {
        assert!(validate_snapshot_id("abcdef0").is_err());
        assert!(validate_snapshot_id("").is_err());
    }

    #[test]
    fn rejects_id_longer_than_64_chars() {
        let id = "a".repeat(65);
        assert!(validate_snapshot_id(&id).is_err());
    }

    #[test]
    fn rejects_non_hex_characters() {
        assert!(validate_snapshot_id("abcdefgh").is_err());
        assert!(validate_snapshot_id("abcdef0!").is_err());
        assert!(validate_snapshot_id("abcdef0 ").is_err());
    }

    #[test]
    fn rejects_path_traversal_attempt() {
        assert!(validate_snapshot_id("../../etc/p").is_err());
    }

    // ── build_retention_args ────────────────────────────────────────────────

    fn all_retention() -> RetentionPolicy {
        RetentionPolicy {
            keep_last: Some(3),
            keep_daily: Some(7),
            keep_weekly: Some(4),
            keep_monthly: Some(12),
            keep_yearly: Some(2),
        }
    }

    #[test]
    fn retention_args_always_starts_with_forget_prune_json() {
        let args = build_retention_args(&[], &[], &all_retention());
        assert_eq!(&args[..3], &["forget", "--prune", "--json"]);
    }

    #[test]
    fn retention_args_with_tags_uses_group_by_not_path() {
        let tags = vec!["home".to_string(), "work".to_string()];
        let paths = vec!["/some/path".to_string()];
        let args = build_retention_args(&tags, &paths, &all_retention());
        assert!(args.contains(&"--group-by".to_string()));
        assert!(args.contains(&"tags".to_string()));
        assert!(args.contains(&"--tag".to_string()));
        assert!(args.contains(&"home,work".to_string()));
        assert!(!args.contains(&"--path".to_string()));
    }

    #[test]
    fn retention_args_without_tags_uses_paths() {
        let paths = vec!["/home/user".to_string(), "/etc".to_string()];
        let args = build_retention_args(&[], &paths, &all_retention());
        assert!(!args.contains(&"--group-by".to_string()));
        assert!(!args.contains(&"--tag".to_string()));
        let path_positions: Vec<usize> = args
            .windows(2)
            .enumerate()
            .filter_map(|(i, w)| if w[0] == "--path" { Some(i) } else { None })
            .collect();
        assert_eq!(path_positions.len(), 2);
        assert_eq!(args[path_positions[0] + 1], "/home/user");
        assert_eq!(args[path_positions[1] + 1], "/etc");
    }

    #[test]
    fn retention_args_emits_all_keep_flags_when_set() {
        let args = build_retention_args(&[], &[], &all_retention());
        let pairs: Vec<(&str, &str)> = args
            .windows(2)
            .map(|w| (w[0].as_str(), w[1].as_str()))
            .collect();
        assert!(pairs.contains(&("--keep-last", "3")));
        assert!(pairs.contains(&("--keep-daily", "7")));
        assert!(pairs.contains(&("--keep-weekly", "4")));
        assert!(pairs.contains(&("--keep-monthly", "12")));
        assert!(pairs.contains(&("--keep-yearly", "2")));
    }

    #[test]
    fn retention_args_omits_none_fields() {
        let policy = RetentionPolicy {
            keep_last: Some(5),
            keep_daily: None,
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        };
        let args = build_retention_args(&[], &[], &policy);
        assert!(args.contains(&"--keep-last".to_string()));
        assert!(!args.contains(&"--keep-daily".to_string()));
        assert!(!args.contains(&"--keep-weekly".to_string()));
        assert!(!args.contains(&"--keep-monthly".to_string()));
        assert!(!args.contains(&"--keep-yearly".to_string()));
    }

    #[test]
    fn retention_args_empty_retention_produces_no_keep_flags() {
        let policy = RetentionPolicy {
            keep_last: None,
            keep_daily: None,
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        };
        let args = build_retention_args(&[], &[], &policy);
        assert_eq!(args, vec!["forget", "--prune", "--json"]);
    }

    // ── parse_diff_output ────────────────────────────────────────────────────

    #[test]
    fn parse_diff_output_maps_each_prefix_to_its_change_type() {
        let stdout = "+  /home/new.txt\n-  /home/gone.txt\nM  /home/changed.txt\nT  /home/retyped.txt\n";
        let result = parse_diff_output(stdout);
        assert_eq!(result.total_added, 1);
        assert_eq!(result.total_removed, 1);
        assert_eq!(result.total_modified, 2); // M and T both count as "modified"
        assert_eq!(result.entries.len(), 4);
        assert_eq!(result.entries[0].change, "added");
        assert_eq!(result.entries[0].path, "/home/new.txt");
        assert_eq!(result.entries[1].change, "removed");
        assert_eq!(result.entries[2].change, "modified");
        assert_eq!(result.entries[3].change, "modified");
        assert!(!result.truncated);
    }

    #[test]
    fn parse_diff_output_skips_unrecognized_lines() {
        let stdout = "+  /home/new.txt\nsome unrelated restic banner line\n?  /home/unknown.txt\n";
        let result = parse_diff_output(stdout);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.total_added, 1);
        assert_eq!(result.total_removed, 0);
        assert_eq!(result.total_modified, 0);
    }

    #[test]
    fn parse_diff_output_trims_path_whitespace() {
        let result = parse_diff_output("+   /home/padded.txt  \n");
        assert_eq!(result.entries[0].path, "/home/padded.txt");
    }

    #[test]
    fn parse_diff_output_empty_input_is_all_zero_and_not_truncated() {
        let result = parse_diff_output("");
        assert_eq!(result.entries.len(), 0);
        assert_eq!(result.total_added, 0);
        assert_eq!(result.total_removed, 0);
        assert_eq!(result.total_modified, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn parse_diff_output_caps_entries_at_limit_but_keeps_full_totals() {
        // DIFF_ENTRY_LIMIT is 500 — feed 501 added lines.
        let stdout = (0..501).map(|i| format!("+  /home/file{i}.txt\n")).collect::<String>();
        let result = parse_diff_output(&stdout);
        assert_eq!(result.entries.len(), 500);
        assert_eq!(result.total_added, 501);
        assert!(result.truncated);
    }

    // ── mirror queue ─────────────────────────────────────────────────────────

    use super::mirror_pair_matches;
    use crate::commands::cache::MirrorEntry;
    use crate::tasks::new_task_slot;

    fn test_mirror_entry(src_id: &str, dest_id: &str) -> MirrorEntry {
        MirrorEntry {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_slot: new_task_slot(),
            cancel_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
            started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            child: std::sync::Arc::new(std::sync::Mutex::new(None)),
            src_id: src_id.to_string(),
            dest_id: dest_id.to_string(),
        }
    }

    #[test]
    fn mirror_pair_matches_rejects_only_exact_src_and_dest_repeat() {
        let entry = test_mirror_entry("repo-a", "repo-b");
        // Exact repeat — this is the case mirror_repo rejects with
        // MIRROR_ALREADY_ACTIVE_ERROR.
        assert!(mirror_pair_matches(&entry, "repo-a", "repo-b"));
        // Same source, different destination — a distinct mirror, must queue.
        assert!(!mirror_pair_matches(&entry, "repo-a", "repo-c"));
        // Different source, same destination — also a distinct mirror, must queue.
        assert!(!mirror_pair_matches(&entry, "repo-d", "repo-b"));
        // Neither matches.
        assert!(!mirror_pair_matches(&entry, "repo-x", "repo-y"));
    }

    /// `notify_one`'s stored-permit semantics, exercised directly against the same
    /// `tokio::select! { biased; cancel_notify.notified() ; turn.lock() }` shape
    /// `mirror_repo`'s spawned task uses to wait its turn — mirrors browse.rs's
    /// `queued_batch_cancel_notify_before_wait_is_not_lost` test for the identical
    /// index-batch pattern. Proves a mirror cancelled while still queued reliably
    /// bails out of the wait rather than risking a lost wakeup.
    #[tokio::test]
    async fn queued_mirror_cancel_notify_before_wait_is_not_lost() {
        let cancel_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Cancel arrives before anyone is waiting.
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        cancel_notify.notify_one();

        let turn = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let _held = turn.lock().await; // turn stays held for the whole test

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::select! {
                biased;
                _ = cancel_notify.notified() => "cancelled",
                _permit = turn.lock() => "acquired",
            }
        })
        .await
        .expect("a notify sent before .notified() is awaited must not be lost");
        assert_eq!(result, "cancelled");
    }

    /// A mirror parked waiting for its turn must cancel immediately once notified,
    /// without waiting for the run ahead of it to release the turn — mirrors
    /// browse.rs's equivalent queued-batch-cancel test.
    #[tokio::test]
    async fn queued_mirror_cancels_immediately_without_waiting_for_turn_release() {
        let turn = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let _held_turn = turn.lock().await; // simulates another mirror currently running

        let cancel_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let turn2 = std::sync::Arc::clone(&turn);
        let notify2 = std::sync::Arc::clone(&cancel_notify);
        let waiter = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = notify2.notified() => "cancelled",
                _permit = turn2.lock() => "acquired",
            }
        });

        // Give the waiter a moment to reach the select!, then cancel it while the
        // turn is still held elsewhere — it must not have to wait for release.
        tokio::task::yield_now().await;
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        cancel_notify.notify_one();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("waiter must resolve promptly, not wait for the held turn to release")
            .unwrap();
        assert_eq!(result, "cancelled");
    }
}
