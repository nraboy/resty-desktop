use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, MasterKey, RestoreHandle};
use super::repo::{run_restic_blocking, run_restic_with_path};
use super::repo_locks::RepoLocks;
use super::snapshot::validate_snapshot_id;
use super::NoConsole;
use crate::tasks::{emit_cancelling, new_task_slot, OperationCtx, TaskKind, TaskOrigin, TaskProgress};

/// Sentinel error returned by `index_snapshots_batch` when `repo_id` already has a batch
/// queued or running (see that function's doc comment) — matched exactly by the frontend's
/// `INDEX_BATCH_ALREADY_ACTIVE_ERROR` (lib/types.ts), same pattern as
/// `CANCELLED_BACKUP_ERROR` (snapshot.rs). Distinguishable from a real failure so the
/// frontend can resync (re-adopt the real batch's state via `get_active_index_batch`)
/// instead of showing "Failed to start indexing" for a request that was never actually
/// attempted.
pub(crate) const INDEX_BATCH_ALREADY_ACTIVE_ERROR: &str = "IndexBatchAlreadyActive";

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
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
    snapshot_id: String,
    path: Option<String>,
) -> Result<Vec<FileEntry>, String> {
    validate_snapshot_id(&snapshot_id)?;
    if let Some(cached) = db.get(&repo_id, &snapshot_id, path.as_deref())? {
        // Cache hit — no restic call happens, so there's no operation to report.
        return Ok(cached);
    }

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Browse,
        repo_id,
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        None,
    );

    let mut args = vec!["ls".to_string(), "--json".to_string(), snapshot_id.clone()];
    if let Some(ref p) = path {
        args.push(p.clone());
    }

    let _rg = repo_locks.read(&repo.path);
    let result = run_restic_blocking(repo, args, restic_path).await;
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    let stdout = result?;

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
    app: tauri::AppHandle,
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
    let task_ctx = OperationCtx::new(
        app,
        TaskKind::RestorePath,
        repo_id,
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        None,
    );

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

    // Isolated in a closure (rather than `?`-returning straight out of the command)
    // so both this and restic_result's outcome can be combined into one `result`
    // before reporting the task bus's terminal phase below — matches the original
    // priority exactly: a strip error wins over restic_result either way.
    let strip_result: Result<(), String> = if strip_leading_path {
        (|| -> Result<(), String> {
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
            Ok(())
        })()
    } else {
        Ok(())
    };

    let result = strip_result.and(restic_result);
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreProgress {
    pub repo_id: String,
    pub snapshot_id: String,
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
    let repo_id_inner = repo_id.clone();

    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Restore,
        repo_id.clone(),
        Some(snapshot_id.clone()),
        TaskOrigin::Manual,
        Some(restore_handle.current_task.clone()),
    );
    let task_progress_inner = task_ctx.progress_emitter();

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

    let result: Result<(), String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut cmd = std::process::Command::new(&restic_path);
        cmd.args(["restore", &snapshot_id, "--target", &target_dir, "--json"])
            .env("RESTIC_REPOSITORY", &repo_path);
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
                        repo_id: repo_id_inner.clone(),
                        snapshot_id: snapshot_id.clone(),
                        percent_done: v["percent_done"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0),
                        files_restored: v["files_restored"].as_u64().unwrap_or(0),
                        total_files: v["total_files"].as_u64().unwrap_or(0),
                        bytes_restored: v["bytes_restored"].as_u64().unwrap_or(0),
                        total_bytes: v["total_bytes"].as_u64().unwrap_or(0),
                        seconds_elapsed: v["seconds_elapsed"].as_u64().unwrap_or(0),
                    };
                    let _ = app.emit("restore:progress", &progress);
                    task_progress_inner.emit(TaskProgress {
                        percent_done: Some(progress.percent_done),
                        items_done: Some(progress.files_restored),
                        items_total: Some(progress.total_files),
                        bytes_done: Some(progress.bytes_restored),
                        bytes_total: Some(progress.total_bytes),
                        label: None,
                        seconds_elapsed: Some(progress.seconds_elapsed),
                        seconds_remaining: None,
                        current_files: None,
                        repo_id: None,
                    });
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
    .map_err(|e| e.to_string())?;

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if restore_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[tauri::command]
pub async fn cancel_restore(app: tauri::AppHandle, restore_handle: State<'_, RestoreHandle>) -> Result<(), String> {
    emit_cancelling(&app, &restore_handle.current_task);
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

/// Marks `manual_active` for the lifetime of a manual indexing run (single,
/// or batch — including time spent merely queued on `IndexHandle::batch_turn`)
/// so the cache_warmer auto-indexer knows to pause. Reference-counted rather
/// than a plain flag: a queued batch and the batch ahead of it in `batch_turn`
/// each hold their own guard concurrently, and a plain bool would let the
/// front batch's `Drop` clear it while the queued one is still waiting.
/// Created inside the spawned task so it stays set for the whole run and
/// decrements on every exit path, including per-snapshot errors.
struct ManualIndexGuard(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl ManualIndexGuard {
    fn new(flag: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        flag.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(flag)
    }
}
impl Drop for ManualIndexGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Deregisters a batch's entry from `IndexHandle::batches` on every exit path (mirrors
/// `ManualIndexGuard`'s Drop pattern) so `cancel_index_batch` can never target a batch
/// that has already finished — see `IndexHandle::batches`' doc comment (cache.rs).
struct BatchDeregisterGuard {
    registry: std::sync::Arc<std::sync::Mutex<HashMap<String, super::cache::BatchCancel>>>,
    operation_id: String,
}
impl Drop for BatchDeregisterGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.lock() {
            map.remove(&self.operation_id);
        }
    }
}

/// A batch's `task_slot` carries its `TaskRef` (identity, including `repo_id`) for as long
/// as it's registered in `IndexHandle::batches` — from the moment it's created (queued) until
/// it deregisters on a terminal phase (see `BatchDeregisterGuard`). Used by both the
/// duplicate-request guard in `index_snapshots_batch` and `get_active_index_batch` below, so
/// the two can never disagree about what counts as "this repo already has a batch".
fn batch_matches_repo(entry: &super::cache::BatchCancel, repo_id: &str) -> bool {
    entry
        .task_slot
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|r| r.repo_id == repo_id))
        .unwrap_or(false)
}

fn repo_has_active_batch(
    batches: &std::sync::Mutex<HashMap<String, super::cache::BatchCancel>>,
    repo_id: &str,
) -> bool {
    match batches.lock() {
        Ok(map) => map.values().any(|entry| batch_matches_repo(entry, repo_id)),
        Err(_) => false,
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

        let task_ctx = OperationCtx::new(
            app.clone(),
            TaskKind::Index,
            repo_id.clone(),
            Some(snapshot_id.clone()),
            TaskOrigin::Manual,
            None,
        );

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

        if ok {
            task_ctx.finished();
        } else {
            task_ctx.failed("Indexing failed");
        }

        if !ok {
            let _ = app2
                .state::<AppDb>()
                .set_browse_status(&repo_id, &snapshot_id, "pending");
        }
    });

    Ok(true)
}

/// Sequentially index a batch of snapshots in a single repo ("Index All").
/// Runs one `run_full_index` at a time (bounding memory to a single
/// snapshot's file list — see module docs for the concurrent-indexing crash
/// this replaces), pauses the auto-indexer for the duration via
/// `IndexHandle::manual_active`, and takes `IndexHandle::gate` around each
/// snapshot so it can never overlap with an auto-indexed one. Fire-and-forget:
/// returns immediately; progress is reported two ways on the `task` bus — a
/// per-snapshot event (`kind: index`, `targetId` = snapshot id, no slot) for
/// the frontend's per-row status UI, and a single batch-level op (`kind:
/// index`, no `targetId`) that emits `progress` with `itemsDone`/`itemsTotal`
/// as snapshots complete, so the Activity panel can show batch progress
/// independent of any page being mounted. The batch-level op's cancel flag and
/// task slot are freshly created per call and registered in
/// `IndexHandle::batches` under its own operationId (not shared across
/// batches — see that field's doc comment), so multiple "Index All" runs
/// (e.g. for different repos) proceed and cancel fully independently; the
/// actual `restic` calls still only ever run one at a time app-wide via the
/// shared `IndexHandle::gate`. A snapshot that fails to index does not abort
/// the batch — the loop continues to the next one. Cancellable between
/// snapshots via `cancel_index_batch(operation_id)`.
///
/// Before entering its per-snapshot loop, the batch first acquires
/// `IndexHandle::batch_turn` — held for the batch's entire duration — so that
/// concurrent batches (e.g. different repos) complete one at a time in the
/// order they started, instead of round-robin-interleaving their snapshots
/// against each other (which is what happens if each batch only takes `gate`
/// per-snapshot with no outer ordering). A batch waiting on `batch_turn` is
/// "queued": it acquires this via a cancellable `tokio::select!` against its
/// own `cancel_notify`, so clicking Stop on a still-queued batch cancels it
/// immediately rather than after the batch ahead of it finishes. The
/// batch-level op is created via `OperationCtx::new_pending` (emits `pending`,
/// not `started`) so a queued batch shows up — and is cancellable — before it
/// ever runs; `.activate()` emits `started` the moment it wins its turn,
/// letting the frontend move it from an "Up Next" area into Active Tasks (see
/// `activity.tsx`'s `reduceIndexBatches`).
///
/// A repo can only ever have one batch registered at a time: if `repo_id` already
/// has a queued or running batch (per `repo_has_active_batch`), this call rejects
/// with `INDEX_BATCH_ALREADY_ACTIVE_ERROR` — the existing batch is left exactly as
/// it was, nothing new is spawned or emitted. This is the authoritative guard
/// against duplicate "Index All" requests for the same repo (e.g. a user clicking
/// the trigger button again while unaware one is already queued); `get_active_index_batch`
/// exists so a frontend can check first and avoid the round-trip, but the backend
/// enforces this regardless of what the frontend does or knows. An explicit `Err`
/// (not a silent `Ok(())`) matches every other "already in progress" guard in this
/// codebase (`BackupHandle`/`RestoreHandle`/`CopyHandle`/`MirrorHandle`/`PruneHandle`'s
/// busy flags) — the sentinel just lets a caller distinguish "nothing happened
/// because one was already running" from a genuine failure, the way
/// `CANCELLED_BACKUP_ERROR` (snapshot.rs) distinguishes a cancel from a real error.
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

    // Reject a duplicate request — see this function's doc comment. Checked before any
    // DB/master-key work so a duplicate call costs nothing. Narrow, practically-irrelevant
    // race: two calls arriving within the same instant, before either's spawned task has
    // registered in `batches`, could theoretically both pass this check — the same tolerance
    // `IndexHandle::batch_turn`'s doc comment already accepts for approximate-FIFO ordering
    // under human-paced clicks, not a correctness issue worth an app-wide lock for.
    if repo_has_active_batch(&index_handle.batches, &repo_id) {
        return Err(INDEX_BATCH_ALREADY_ACTIVE_ERROR.to_string());
    }

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let manual_active = std::sync::Arc::clone(&index_handle.manual_active);
    let gate = std::sync::Arc::clone(&index_handle.gate);
    let batch_turn = std::sync::Arc::clone(&index_handle.batch_turn);
    let batches_registry = std::sync::Arc::clone(&index_handle.batches);

    let batch_total = snapshot_ids.len() as u64;

    tauri::async_runtime::spawn(async move {
        // First, so a batch that ends up merely queued on `batch_turn` below still
        // keeps the auto-indexer paused for the whole time it's waiting — see
        // ManualIndexGuard's doc comment on why this is refcounted.
        let _guard = ManualIndexGuard::new(manual_active);

        // Fresh per-batch cancel flag + task slot + cancel notify (not
        // IndexHandle-shared — see IndexHandle::batches' doc comment). No targetId
        // on the batch op itself (that's what lets per-snapshot events and this one
        // be told apart on the bus). Registered under this batch's own operationId
        // so cancel_index_batch can target exactly this run, independent of any
        // other concurrent batch.
        let batch_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let batch_slot = new_task_slot();
        let cancel_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let batch_started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Cloned now, before the loop below consumes `snapshot_ids` via `.into_iter()` —
        // this is the immutable record `get_active_index_batch` hands back to a page that
        // just (re)mounted, so it can restore exactly which snapshots this batch covers.
        let target_ids = std::sync::Arc::new(snapshot_ids.clone());
        // Pending, not Started — this batch hasn't begun indexing yet, it's about
        // to wait for its turn on batch_turn below. Registering + emitting Pending
        // here (before that wait) is what makes a queued batch visible in the
        // Activity panel's "Up Next" area and cancellable via cancel_index_batch
        // while it's still waiting.
        let batch_ctx = OperationCtx::new_pending(
            app.clone(),
            TaskKind::Index,
            repo_id.clone(),
            None,
            TaskOrigin::Manual,
            Some(batch_slot.clone()),
        );
        let operation_id = batch_ctx.operation_id().to_string();
        // Graceful on a poisoned lock, matching BatchDeregisterGuard/cancel_index_batch below —
        // if registration is skipped, cancel_index_batch simply won't find this batch (a no-op,
        // not a panic), so the batch still runs to completion, just uncancellable.
        if let Ok(mut map) = batches_registry.lock() {
            map.insert(
                operation_id.clone(),
                super::cache::BatchCancel {
                    cancel: batch_cancel.clone(),
                    task_slot: batch_slot,
                    cancel_notify: cancel_notify.clone(),
                    started: batch_started.clone(),
                    target_ids: target_ids.clone(),
                },
            );
        }
        let _dereg = BatchDeregisterGuard { registry: batches_registry.clone(), operation_id };

        // Wait for this batch's turn so concurrent batches complete in start order
        // rather than round-robin (see IndexHandle::batch_turn's doc comment). This
        // is cancellable: a batch parked here (already registered above, so its Stop
        // button works) bails out the moment cancel_index_batch fires cancel_notify,
        // instead of waiting for the batch ahead of it to finish. No separate
        // pre-check against `batch_cancel` is needed even if it was already set the
        // instant we got here — `notify_one()`'s stored-permit semantics mean
        // `cancel_notify.notified()` below resolves immediately on its very first
        // poll in that case, and `biased` guarantees the cancel branch is checked
        // before `batch_turn.lock()` (proven by
        // `queued_batch_cancel_notify_before_wait_is_not_lost` below).
        let _turn = tokio::select! {
            biased;
            _ = cancel_notify.notified() => {
                batch_ctx.cancelled();
                return;
            }
            permit = batch_turn.lock() => permit,
        };
        // Won the turn — this batch is now actually running. Emits Started, which
        // promotes it from the Activity panel's "Up Next" area to Active Tasks.
        batch_ctx.activate();
        batch_started.store(true, std::sync::atomic::Ordering::SeqCst);

        let batch_progress = batch_ctx.progress_emitter();

        for (i, snapshot_id) in snapshot_ids.into_iter().enumerate() {
            if batch_cancel.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }

            // Labeled block (same idiom as prune_all_repos/prune_repo — see CLAUDE.md's
            // Operation Event Bus section) so every exit path — each early `break 'work`
            // below, or falling off the end after real indexing work — reaches the single
            // progress emit after it, with no separate "final bump" needed: the last
            // iteration's emit always reports `i + 1 == batch_total` by construction,
            // whether that snapshot was actually indexed or skipped as already-complete.
            'work: {
                let db_outer = app.state::<AppDb>();
                let status_map = match db_outer.get_browse_status(&repo_id) {
                    Ok(m) => m,
                    Err(_) => break 'work,
                };
                if matches!(
                    status_map.get(&snapshot_id).map(|s| s.as_str()),
                    Some("complete") | Some("in_progress")
                ) {
                    break 'work;
                }
                if db_outer
                    .set_browse_status(&repo_id, &snapshot_id, "in_progress")
                    .is_err()
                {
                    break 'work;
                }

                // Slot is None here — the batch op above owns current_task, so a cancel
                // during this snapshot's indexing emits `cancelling` on the batch, not this
                // per-snapshot op (which has no cancel affordance of its own).
                let task_ctx = OperationCtx::new(
                    app.clone(),
                    TaskKind::Index,
                    repo_id.clone(),
                    Some(snapshot_id.clone()),
                    TaskOrigin::Manual,
                    None,
                );

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

                if ok {
                    task_ctx.finished();
                } else {
                    task_ctx.failed("Indexing failed");
                }

                if !ok {
                    let _ = app
                        .state::<AppDb>()
                        .set_browse_status(&repo_id, &snapshot_id, "pending");
                }
            }

            batch_progress.emit(TaskProgress {
                items_done: Some((i + 1) as u64),
                items_total: Some(batch_total),
                ..Default::default()
            });
        }

        if batch_cancel.load(std::sync::atomic::Ordering::SeqCst) {
            batch_ctx.cancelled();
        } else {
            batch_ctx.finished();
        }
    });

    Ok(())
}

/// Signals a specific in-progress `index_snapshots_batch` run (identified by its
/// batch-level operationId — see that function's doc comment) to stop after the
/// currently-indexing snapshot finishes. A no-op, not an error, if that batch has
/// already finished (or `operation_id` doesn't match any running batch) — mirrors
/// the old "has no effect if no batch is running" behavior, now scoped per-batch
/// instead of app-wide.
#[tauri::command]
pub fn cancel_index_batch(
    app: tauri::AppHandle,
    index_handle: State<'_, super::cache::IndexHandle>,
    operation_id: String,
) -> Result<(), String> {
    let entry = index_handle
        .batches
        .lock()
        .map_err(|e| e.to_string())?
        .get(&operation_id)
        .cloned();
    if let Some(entry) = entry {
        emit_cancelling(&app, &entry.task_slot);
        entry.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        // Wakes the batch if it's currently parked waiting for its turn on
        // batch_turn, so a queued batch cancels immediately instead of waiting
        // for the batch ahead of it to finish. notify_one()'s stored-permit
        // semantics make this race-free even if the batch hasn't started
        // waiting yet (or has already moved past the wait and is indexing —
        // in that case this permit is simply never consumed, a harmless no-op).
        entry.cancel_notify.notify_one();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveIndexBatchStatus {
    pub operation_id: String,
    /// False while still queued waiting for `IndexHandle::batch_turn`; true once it's won
    /// its turn and is actually indexing. Mirrors the `pending`/`started` task-bus phases —
    /// see `BatchCancel::started`'s doc comment (cache.rs).
    pub started: bool,
    /// The exact snapshot ids this batch covers (see `BatchCancel::target_ids`) — lets a
    /// page that just (re)mounted cross-reference against its own already-fetched
    /// index-status map to restore accurate local progress instead of only knowing "a
    /// batch exists" with no way to know what it's actually working on.
    pub target_ids: Vec<String>,
}

/// Testable core of `get_active_index_batch` below, split out so it can be exercised with a
/// plain `Mutex<HashMap<...>>` in unit tests without constructing a `tauri::State`
/// (matching the pattern used elsewhere in this codebase — see e.g. `repo.rs`'s
/// `set_restic_path` doc comment).
fn find_active_batch_for_repo(
    batches: &std::sync::Mutex<HashMap<String, super::cache::BatchCancel>>,
    repo_id: &str,
) -> Result<Option<ActiveIndexBatchStatus>, String> {
    let map = batches.lock().map_err(|e| e.to_string())?;
    for (operation_id, entry) in map.iter() {
        if batch_matches_repo(entry, repo_id) {
            return Ok(Some(ActiveIndexBatchStatus {
                operation_id: operation_id.clone(),
                started: entry.started.load(std::sync::atomic::Ordering::SeqCst),
                target_ids: entry.target_ids.as_ref().clone(),
            }));
        }
    }
    Ok(None)
}

/// Reports whether `repo_id` already has an "Index All" batch queued or running, via the
/// same `IndexHandle::batches` lookup `index_snapshots_batch` uses to silently drop a
/// duplicate request (see that function's doc comment). Lets a page that just (re)mounted —
/// and so missed the live `pending`/`started` task-bus events for a batch that was started
/// before it mounted (e.g. the user navigated away and back) — restore the correct
/// queued/running UI state instead of only ever learning about it from events observed
/// since mount. Sync, no restic/DB work — same category as `get_snapshot_index_status`.
#[tauri::command]
pub fn get_active_index_batch(
    index_handle: State<'_, super::cache::IndexHandle>,
    repo_id: String,
) -> Result<Option<ActiveIndexBatchStatus>, String> {
    find_active_batch_for_repo(&index_handle.batches, &repo_id)
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

    // ── IndexHandle::batches registry / BatchDeregisterGuard ──────────────────
    // Covers the concurrency bookkeeping index_snapshots_batch/cancel_index_batch rely on:
    // per-operationId isolation (the bug this registry replaced — see IndexHandle::batches'
    // doc comment, cache.rs) and deregistration on drop (so cancel_index_batch can never find
    // a batch that has already finished).

    fn make_batch_cancel() -> crate::commands::cache::BatchCancel {
        crate::commands::cache::BatchCancel {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            task_slot: new_task_slot(),
            cancel_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
            started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            target_ids: std::sync::Arc::new(Vec::new()),
        }
    }

    /// Like `make_batch_cancel`, but with `task_slot` populated as if a real batch had
    /// registered for `repo_id` — `batch_matches_repo`/`repo_has_active_batch`/
    /// `find_active_batch_for_repo` all key off the slot's `TaskRef.repo_id`, not the
    /// `batches` map key, so exercising them needs a populated slot. `target_ids` is
    /// almost always irrelevant to the test at hand, hence a slice (usually empty) rather
    /// than a required `Vec`.
    fn make_batch_cancel_for_repo(
        repo_id: &str,
        started: bool,
        target_ids: &[&str],
    ) -> crate::commands::cache::BatchCancel {
        let mut entry = make_batch_cancel();
        *entry.task_slot.lock().unwrap() = Some(crate::tasks::TaskRef {
            operation_id: "op".to_string(),
            kind: crate::tasks::TaskKind::Index,
            repo_id: repo_id.to_string(),
            target_id: None,
            origin: crate::tasks::TaskOrigin::Manual,
        });
        entry.started.store(started, std::sync::atomic::Ordering::SeqCst);
        entry.target_ids = std::sync::Arc::new(target_ids.iter().map(|s| s.to_string()).collect());
        entry
    }

    /// Mirrors `cancel_index_batch`'s exact lookup-then-store-then-notify pattern
    /// (`.get(&operation_id).cloned()`, then act on the *fetched* clone) rather than
    /// acting on a pre-insertion reference — so these tests exercise the same path a
    /// regression at the real call site would break, instead of just exercising
    /// HashMap/AtomicBool semantics. Returns whether a matching batch was found
    /// (mirrors the real command's no-op-on-miss).
    fn simulate_cancel(
        registry: &std::sync::Arc<std::sync::Mutex<HashMap<String, crate::commands::cache::BatchCancel>>>,
        operation_id: &str,
    ) -> bool {
        let entry = registry.lock().unwrap().get(operation_id).cloned();
        match entry {
            Some(entry) => {
                entry.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                entry.cancel_notify.notify_one();
                true
            }
            None => false,
        }
    }

    #[test]
    fn cancel_targets_only_the_matching_operation_id() {
        let index_handle = crate::commands::cache::IndexHandle::new();
        index_handle.batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel());
        index_handle.batches.lock().unwrap().insert("batchB".to_string(), make_batch_cancel());

        // Cancel batchA the same way cancel_index_batch does: look it up by id, act on the
        // fetched clone. If a regression reused one shared BatchCancel across every insert
        // (the historical bug), or `.get` returned the wrong entry, batchB's flag re-fetched
        // fresh from the map below would incorrectly read `true` too.
        assert!(simulate_cancel(&index_handle.batches, "batchA"));

        let map = index_handle.batches.lock().unwrap();
        assert!(map.get("batchA").unwrap().cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!map.get("batchB").unwrap().cancel.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn batch_deregister_guard_removes_entry_on_drop() {
        let index_handle = crate::commands::cache::IndexHandle::new();
        let registry = std::sync::Arc::clone(&index_handle.batches);
        registry.lock().unwrap().insert("batchA".to_string(), make_batch_cancel());
        assert!(registry.lock().unwrap().contains_key("batchA"));

        {
            let _guard = BatchDeregisterGuard {
                registry: registry.clone(),
                operation_id: "batchA".to_string(),
            };
        } // guard dropped here

        assert!(!registry.lock().unwrap().contains_key("batchA"));
    }

    #[test]
    fn batch_deregister_guard_leaves_other_entries_untouched() {
        let index_handle = crate::commands::cache::IndexHandle::new();
        let registry = std::sync::Arc::clone(&index_handle.batches);
        registry.lock().unwrap().insert("batchA".to_string(), make_batch_cancel());
        registry.lock().unwrap().insert("batchB".to_string(), make_batch_cancel());

        {
            let _guard = BatchDeregisterGuard {
                registry: registry.clone(),
                operation_id: "batchA".to_string(),
            };
        }

        let map = registry.lock().unwrap();
        assert!(!map.contains_key("batchA"));
        assert!(map.contains_key("batchB"));
    }

    #[test]
    fn cancel_for_unknown_operation_id_is_a_noop() {
        // A miss (e.g. the batch already finished and deregistered, or a stale/garbage id)
        // must be a clean no-op, not a panic — and must not disturb any other running batch.
        let index_handle = crate::commands::cache::IndexHandle::new();
        index_handle.batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel());

        assert!(!simulate_cancel(&index_handle.batches, "unknown-id"));

        let map = index_handle.batches.lock().unwrap();
        assert!(!map.get("batchA").unwrap().cancel.load(std::sync::atomic::Ordering::SeqCst));
    }

    // ── IndexHandle::batch_turn cancellable acquisition ────────────────────────
    // Covers the new queued-batch-cancels-promptly logic in index_snapshots_batch:
    // a batch parked waiting for batch_turn must bail out via cancel_notify the
    // moment cancel_index_batch fires, without ever waiting for the batch ahead of
    // it to release the turn. Mirrors the real `select! { biased; cancel_notify.notified() ,
    // batch_turn.lock() }` shape.

    /// Cancelling a batch that is *already parked* waiting on `batch_turn` (the
    /// notify fires after the task has started `.notified()`) must resolve via the
    /// cancel branch, not the lock branch — proven by asserting the turn is never
    /// actually acquired.
    #[tokio::test]
    async fn queued_batch_cancels_promptly_when_notified_while_waiting() {
        let batch_turn = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let cancel_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Simulate another batch currently holding the turn.
        let _held = batch_turn.lock().await;

        let turn2 = batch_turn.clone();
        let notify2 = cancel_notify.clone();
        let cancel2 = cancel.clone();
        let waiter = tokio::spawn(async move {
            if cancel2.load(std::sync::atomic::Ordering::SeqCst) {
                return "cancelled-precheck";
            }
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

    /// `notify_one`'s stored-permit semantics: if the notify arrives *before* the
    /// batch starts waiting (a lost-wakeup risk with a naive Condvar-style signal),
    /// the subsequent `.notified()` call must still resolve immediately rather than
    /// blocking forever.
    #[tokio::test]
    async fn queued_batch_cancel_notify_before_wait_is_not_lost() {
        let cancel_notify = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Cancel arrives before anyone is waiting.
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        cancel_notify.notify_one();

        let batch_turn = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        let _held = batch_turn.lock().await; // turn stays held for the whole test

        // The real call site's pre-check would short-circuit here; this test
        // exercises the select! branch directly to prove the stored permit still
        // wakes `.notified()` even though notify_one ran first.
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::select! {
                biased;
                _ = cancel_notify.notified() => "cancelled",
                _permit = batch_turn.lock() => "acquired",
            }
        })
        .await
        .expect("a notify sent before .notified() is awaited must not be lost");
        assert_eq!(result, "cancelled");
    }

    // ── ManualIndexGuard refcount ───────────────────────────────────────────────
    // A queued batch and the batch currently running ahead of it each hold their
    // own ManualIndexGuard concurrently. With a plain bool, the front batch
    // finishing (guard dropped) would clear the flag out from under the still-
    // queued one, letting the auto-indexer slip in between batches. The refcount
    // must stay > 0 until *both* guards have dropped.
    #[test]
    fn manual_active_refcount_stays_set_until_all_guards_drop() {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let running_guard = ManualIndexGuard::new(flag.clone());
        assert_eq!(flag.load(std::sync::atomic::Ordering::SeqCst), 1);

        let queued_guard = ManualIndexGuard::new(flag.clone());
        assert_eq!(flag.load(std::sync::atomic::Ordering::SeqCst), 2);

        drop(running_guard);
        assert_eq!(
            flag.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the still-queued batch's guard must keep the flag set after the front batch finishes"
        );

        drop(queued_guard);
        assert_eq!(flag.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    // ── duplicate-batch guard / get_active_index_batch ─────────────────────────
    // Covers the actual dedup mechanism index_snapshots_batch relies on to silently drop a
    // second "Index All" request for a repo that already has one queued or running, and the
    // query command (get_active_index_batch) that lets a frontend check the same thing
    // in advance instead of relying purely on live task-bus events.

    #[test]
    fn repo_has_active_batch_true_for_a_queued_or_running_match() {
        let batches = std::sync::Mutex::new(HashMap::new());
        batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel_for_repo("repoA", false, &[]));
        assert!(repo_has_active_batch(&batches, "repoA"));

        batches.lock().unwrap().get_mut("batchA").unwrap().started.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(repo_has_active_batch(&batches, "repoA"), "a running (started) batch counts too, not just queued");
    }

    #[test]
    fn repo_has_active_batch_false_for_a_different_repo_or_an_unregistered_slot() {
        let batches = std::sync::Mutex::new(HashMap::new());
        batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel_for_repo("repoA", false, &[]));
        assert!(!repo_has_active_batch(&batches, "repoB"));

        // An entry whose task_slot was never populated (shouldn't happen for a real batch,
        // but matches make_batch_cancel()'s default) must not match any repo.
        batches.lock().unwrap().insert("batchB".to_string(), make_batch_cancel());
        assert!(!repo_has_active_batch(&batches, "repoA_typo_no_match"));
    }

    #[test]
    fn repo_has_active_batch_false_on_an_empty_registry() {
        let batches = std::sync::Mutex::new(HashMap::new());
        assert!(!repo_has_active_batch(&batches, "repoA"));
    }

    #[test]
    fn find_active_batch_for_repo_reports_queued_vs_running() {
        let batches = std::sync::Mutex::new(HashMap::new());
        batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel_for_repo("repoA", false, &[]));

        let status = find_active_batch_for_repo(&batches, "repoA").unwrap();
        let status = status.expect("repoA has a registered batch");
        assert_eq!(status.operation_id, "batchA");
        assert!(!status.started, "queued batch must report started: false");

        batches.lock().unwrap().insert("batchB".to_string(), make_batch_cancel_for_repo("repoB", true, &[]));
        let status = find_active_batch_for_repo(&batches, "repoB").unwrap().unwrap();
        assert_eq!(status.operation_id, "batchB");
        assert!(status.started, "running batch must report started: true");
    }

    #[test]
    fn find_active_batch_for_repo_is_none_for_an_unrelated_repo() {
        let batches = std::sync::Mutex::new(HashMap::new());
        batches.lock().unwrap().insert("batchA".to_string(), make_batch_cancel_for_repo("repoA", false, &[]));
        assert!(find_active_batch_for_repo(&batches, "repoB").unwrap().is_none());
    }

    #[test]
    fn find_active_batch_for_repo_reports_the_batchs_exact_target_ids() {
        let batches = std::sync::Mutex::new(HashMap::new());
        batches.lock().unwrap().insert(
            "batchA".to_string(),
            make_batch_cancel_for_repo("repoA", false, &["snap1", "snap2", "snap3"]),
        );
        let status = find_active_batch_for_repo(&batches, "repoA").unwrap().unwrap();
        assert_eq!(status.target_ids, vec!["snap1", "snap2", "snap3"]);
    }
}
