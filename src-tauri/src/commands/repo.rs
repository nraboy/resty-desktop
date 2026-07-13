use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use super::cache::{AppDb, FullRepository, MasterKey, PruneHandle, Repository};
use super::crypto;
use super::repo_locks::RepoLocks;
use super::NoConsole;
use crate::tasks::{emit_cancelling, OperationCtx, TaskKind, TaskOrigin, TaskProgress};

#[derive(Debug, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
    /// Unix-seconds timestamp of when this value was cached. `None` only for the
    /// pure `parse_stats_json` path (fresh restic output has no such field) —
    /// callers that hand back a `ResticStats` to the frontend always fill it in
    /// from `repo_stats_cache.cached_at` before returning.
    pub cached_at: Option<i64>,
}

/// Finds the last non-blank line in restic `--json` stdout. restic sometimes emits
/// blank/whitespace-only trailing lines; the real JSON payload is always the last
/// non-blank one. Shared by `parse_stats_json` here and `get_snapshot_stats` in
/// snapshot.rs, both of which parse `restic stats --json` output.
pub(crate) fn last_nonblank_line(stdout: &str) -> Option<&str> {
    stdout.lines().rfind(|l| !l.trim().is_empty())
}

/// Parses `restic stats --json` stdout into `ResticStats`. Pure — no restic call, no
/// DB write — so `fetch_and_cache_stats` (the async command wrapper) can be tested by
/// feeding it captured stdout instead of shelling out to a real restic binary.
pub(crate) fn parse_stats_json(stdout: &str) -> Result<ResticStats, String> {
    let last_line = last_nonblank_line(stdout).ok_or_else(|| "No output from restic stats".to_string())?;
    let v: serde_json::Value = serde_json::from_str(last_line).map_err(|e| e.to_string())?;
    Ok(ResticStats {
        total_size: v["total_size"].as_u64().unwrap_or(0),
        total_file_count: v["total_file_count"].as_u64().unwrap_or(0),
        snapshots_count: v["snapshots_count"].as_u64().unwrap_or(0),
        cached_at: None,
    })
}

/// Rejects an empty password for repo *creation* (`init_repo`). Passwordless
/// repos may be opened/imported/exported, but not created from this app. This is
/// a defense-in-depth guard behind the UI (the Init modal already requires a
/// password); extracted as a helper so it can be unit-tested without Tauri state.
/// Uses `is_empty()` to match the codebase's empty-string-means-passwordless
/// convention (see `apply_repo_password`).
pub(crate) fn validate_init_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("A password is required to create a repository.".to_string());
    }
    Ok(())
}

/// Validates a user-supplied restic binary path (already trimmed). Pure — no
/// filesystem I/O beyond checking existence of an already-resolved absolute path, no
/// DB write — so `set_restic_path` can be tested without a `tauri::State<AppDb>`.
pub(crate) fn validate_restic_path(trimmed: &str) -> Result<(), String> {
    if trimmed.is_empty() {
        return Err("Restic path must not be empty".to_string());
    }
    // If the value looks like an absolute path, verify the file exists.
    if (trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.contains(":\\"))
        && !std::path::Path::new(trimmed).is_file() {
            return Err(format!("No file found at '{trimmed}'"));
        }
    Ok(())
}

/// Applies a repo's password to a restic `Command`: a normal password sets
/// `RESTIC_PASSWORD`; an empty stored password means a repo created with
/// `restic init --insecure-no-password`, which restic requires the caller to pass
/// `--insecure-no-password` on every subsequent command (an empty/unset
/// `RESTIC_PASSWORD` alone makes restic prompt interactively, not use no password).
/// Setting both the flag and the env var is a restic error, so the two are mutually
/// exclusive.
pub(crate) fn apply_repo_password(cmd: &mut std::process::Command, password: &str) {
    if password.is_empty() {
        cmd.arg("--insecure-no-password");
    } else {
        cmd.env("RESTIC_PASSWORD", password);
    }
}

/// Same as `apply_repo_password` but for a copy/mirror source repo's `--from-*` flags.
pub(crate) fn apply_from_repo_password(cmd: &mut std::process::Command, password: &str) {
    if password.is_empty() {
        cmd.arg("--from-insecure-no-password");
    } else {
        cmd.env("RESTIC_FROM_PASSWORD", password);
    }
}

pub fn run_restic_with_path(
    repo: &FullRepository,
    args: Vec<&str>,
    restic_path: &str,
) -> Result<String, String> {
    let mut cmd = std::process::Command::new(restic_path);
    cmd.args(args).env("RESTIC_REPOSITORY", &repo.path);
    apply_repo_password(&mut cmd, &repo.password);
    let output = cmd
        .stdin(std::process::Stdio::null())
        .no_console()
        .augment_path()
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| format!("restic output contained invalid UTF-8: {e}"))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// One-shot restic on a blocking-pool thread so it never occupies an async-runtime
/// worker. Owns its inputs so they can cross the spawn_blocking boundary.
pub(crate) async fn run_restic_blocking(
    repo: FullRepository,
    args: Vec<String>,
    restic_path: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_restic_with_path(&repo, arg_refs, &restic_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_repos(db: State<'_, AppDb>) -> Result<Vec<Repository>, String> {
    db.list_repos()
}

#[tauri::command]
pub async fn add_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    id: String,
    name: String,
    path: String,
    password: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let (nonce, ciphertext) = crypto::encrypt(&key, password.as_bytes())?;
    db.add_repo(&id, &name, &path, &nonce, &ciphertext)
}

#[tauri::command]
pub async fn remove_repo(db: State<'_, AppDb>, repo_id: String) -> Result<(), String> {
    db.remove_repo(&repo_id)
}

#[tauri::command]
pub async fn rename_repo(
    db: State<'_, AppDb>,
    repo_id: String,
    new_name: String,
) -> Result<(), String> {
    db.rename_repo(&repo_id, &new_name)
}

#[tauri::command]
pub async fn update_repo_path(
    db: State<'_, AppDb>,
    repo_id: String,
    new_path: String,
) -> Result<(), String> {
    db.update_repo_path(&repo_id, &new_path)
}

#[tauri::command]
pub async fn get_repo_password(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<String, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    Ok(repo.password.clone())
}

#[tauri::command]
pub async fn update_repo_password(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    new_password: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let (nonce, ciphertext) = crypto::encrypt(&key, new_password.as_bytes())?;
    db.update_repo_password(&repo_id, &nonce, &ciphertext)
}

/// Initialise a new restic repository, then persist it.
#[tauri::command]
pub async fn init_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    id: String,
    name: String,
    path: String,
    password: String,
) -> Result<(), String> {
    validate_init_password(&password)?;
    let restic_path = super::get_restic_path(&db);
    let dummy = FullRepository { path: path.clone(), password: password.clone() };
    run_restic_blocking(dummy, vec!["init".into()], restic_path).await.map(|_| ())?;

    let key = master_key.get()?;
    let (nonce, ciphertext) = crypto::encrypt(&key, password.as_bytes())?;
    db.add_repo(&id, &name, &path, &nonce, &ciphertext)
}

/// Test an unsaved repo connection (used by the "Test Connection" button in the add modal).
#[tauri::command]
pub async fn test_repo_connection(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    path: String,
    password: String,
) -> Result<(), String> {
    let restic_path = super::get_restic_path(&db);
    // No saved repoId yet (the repo isn't added until the test passes) — matches
    // prune_all_repos' empty-repoId convention for the same "no single id" case.
    let task_ctx = OperationCtx::new(app, TaskKind::TestConnection, String::new(), None, TaskOrigin::Manual, None);
    let dummy = FullRepository { path, password };
    let result = run_restic_blocking(dummy, vec!["snapshots".into(), "--json".into()], restic_path)
        .await
        .map(|_| ());
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

/// Cache-only read: never shells out to restic, even on a miss. This used to fall through to
/// `fetch_and_cache_stats` on a miss — fine under normal operation (a genuine miss only ever
/// happened for a repo that had literally never been fetched), but "Clear All Cache"
/// (`AppDb::clear_cache`, SettingsPage) wipes `repo_stats_cache` for every repo at once, and
/// RepositoriesPage calls this command for every repo on mount (see CLAUDE.md's Intentional
/// Designs). Together, that meant the very next visit to Repositories after a cache clear
/// silently kicked off a real `restic stats` subprocess for every single repo — a manual-only
/// feature auto-refreshing itself the moment its cache was cleared, contradicting the whole
/// point of the manual-only redesign (see this file's `refresh_repo_stats` doc comment).
/// Returns `Err` on a miss so the frontend's existing "couldn't load" fallback (the `—`
/// placeholder) applies — same as any other failed fetch; the user must click Refresh (row or
/// All) to actually populate it, exactly like a brand-new repo always required. No restic call,
/// no `RepoLocks`/`MasterKey` needed, so this is a plain sync command (matches `list_repos`).
#[tauri::command]
pub fn get_repo_stats(db: State<'_, AppDb>, repo_id: String) -> Result<ResticStats, String> {
    match db.get_stats(&repo_id) {
        Ok(Some((total_size, total_file_count, snapshots_count, cached_at))) => {
            Ok(ResticStats { total_size, total_file_count, snapshots_count, cached_at: Some(cached_at) })
        }
        Ok(None) => Err("No cached stats for this repository".to_string()),
        Err(e) => Err(e),
    }
}

/// Manual-only refresh: stats are never auto-evicted (see CLAUDE.md's Restic
/// Integration section) — this is the sole way a repo's cached stats change,
/// aside from the very first fetch. A failed refresh leaves the last-good
/// cached value (and its `cached_at`) untouched, since `set_stats` only
/// overwrites on a successful fetch below.
#[tauri::command]
pub async fn refresh_repo_stats(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<ResticStats, String> {
    fetch_and_cache_stats(&app, &db, &master_key, &repo_locks, &repo_id).await
}

/// `task_ctx` is created *first*, before any fallible step, and every step below reports
/// through it explicitly (`task_ctx.failed(e)`) rather than via `?` — so every way this can
/// fail (locked app, deleted repo, the restic call itself, a malformed response, the cache
/// write) reliably emits a `task` "failed" event. This matters because the frontend now
/// derives a boolean "last refresh failed" marker purely from the bus (see `activity.tsx`'s
/// `reduceStatsOps`) with no fallback to the invoke promise's own rejection — relying on `?`
/// here would let some failures fall through to `OperationCtx`'s `Drop` backstop (or, for the
/// two steps before `task_ctx` used to exist, emit nothing at all), silently leaving that
/// marker unset even though the refresh genuinely failed.
async fn fetch_and_cache_stats(
    app: &tauri::AppHandle,
    db: &AppDb,
    master_key: &MasterKey,
    repo_locks: &RepoLocks,
    repo_id: &str,
) -> Result<ResticStats, String> {
    let task_ctx = OperationCtx::new(app.clone(), TaskKind::Stats, repo_id, None, TaskOrigin::Manual, None);

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
    let _rg = repo_locks.read(&repo.path);
    let result = run_restic_blocking(repo, vec!["stats".into(), "--json".into()], restic_path).await;
    let stdout = match result {
        Ok(stdout) => stdout,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let mut stats = match parse_stats_json(&stdout) {
        Ok(s) => s,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    // Cache write happens before `finished()` is emitted, on purpose: a `task`-bus
    // consumer that hears "finished" and re-reads `get_repo_stats` must never race
    // ahead of this write. See CLAUDE.md's Operation Event Bus section.
    let ts = match db.set_stats(repo_id, stats.total_size, stats.total_file_count, stats.snapshots_count) {
        Ok(t) => t,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    stats.cached_at = Some(ts);
    task_ctx.finished();
    Ok(stats)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub success: bool,
    pub errors: Vec<String>,
    pub duration_seconds: f64,
}

#[tauri::command]
pub async fn check_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<CheckResult, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(app, TaskKind::Check, repo_id, None, TaskOrigin::Manual, None);

    // `check` is a shared-lock read — register as a reader, held across the
    // spawn_blocking below for the whole child-process lifetime.
    let _rg = repo_locks.read(&repo.path);

    let spawn_result = tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let mut cmd = std::process::Command::new(&restic_path);
        cmd.args(["check", "--json"]).env("RESTIC_REPOSITORY", &repo.path);
        apply_repo_password(&mut cmd, &repo.password);
        let output = cmd
            .stdin(std::process::Stdio::null())
            .no_console()
            .augment_path()
            .output()
            .map_err(|e| format!("Failed to run restic: {e}"))?;
        Ok::<_, String>((output, started.elapsed().as_secs_f64()))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    // A `finished` task means the check *ran*; the pass/fail verdict is part of
    // CheckResult's data, not the task's own outcome. Only a spawn/process failure
    // (couldn't even run restic) is a task-level `failed`.
    match &spawn_result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    let (output, duration_seconds) = spawn_result?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut errors: Vec<String> = Vec::new();

    for line in stdout.lines().chain(stderr.lines()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let msg = match v["message_type"].as_str() {
                Some("error") => v["error"]["message"].as_str().map(str::to_string),
                Some("exit_error") => v["message"].as_str().map(str::to_string),
                _ => None,
            };
            if let Some(m) = msg {
                if !errors.contains(&m) {
                    errors.push(m);
                }
            }
        }
    }

    if !output.status.success() && errors.is_empty() {
        let raw = stderr.trim().to_string();
        if !raw.is_empty() {
            errors.push(raw);
        }
    }

    Ok(CheckResult {
        success: output.status.success(),
        errors,
        duration_seconds,
    })
}

#[tauri::command]
pub fn get_restic_path(db: State<'_, AppDb>) -> Result<String, String> {
    db.get_setting("restic_path", "restic")
}

#[tauri::command]
pub fn set_restic_path(db: State<'_, AppDb>, path: String) -> Result<(), String> {
    let trimmed = path.trim();
    validate_restic_path(trimmed)?;
    db.set_setting("restic_path", trimmed)
}

#[tauri::command]
pub fn get_compression(db: State<'_, AppDb>) -> Result<String, String> {
    db.get_setting("compression", "auto")
}

#[tauri::command]
pub fn set_compression(db: State<'_, AppDb>, value: String) -> Result<(), String> {
    db.set_setting("compression", &value)
}

#[tauri::command]
pub fn get_restore_path(app: tauri::AppHandle, db: State<'_, AppDb>) -> Result<String, String> {
    let stored = db.get_setting("restore_path", "")?;
    if !stored.is_empty() {
        return Ok(stored);
    }
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("Could not determine home directory: {e}"))?;
    let default_path = home.join("restores").to_string_lossy().into_owned();
    db.set_setting("restore_path", &default_path)?;
    Ok(default_path)
}

#[tauri::command]
pub fn set_restore_path(db: State<'_, AppDb>, path: String) -> Result<(), String> {
    db.set_setting("restore_path", path.trim())
}

#[tauri::command]
pub fn get_restic_version(db: State<'_, AppDb>) -> Result<String, String> {
    let restic_path = super::get_restic_path(&db);
    let output = std::process::Command::new(&restic_path)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .no_console()
        .augment_path()
        .output()
        .map_err(|_| format!("restic not found at '{restic_path}'"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Outcome of one `restic prune` attempt (see `run_one_prune_attempt`).
enum PruneAttempt {
    Success,
    Cancelled,
    Failed(String),
}

/// One spawn-poll-capture attempt of `restic prune` against `full`, respecting
/// `prune_handle.cancelled` throughout via responsive `try_wait` polling — factored out here so
/// both call sites, and the retry-on-"already locked" loop each wraps around it, share one
/// implementation instead of duplicating this logic twice. Captures stderr (previously
/// discarded via `Stdio::null()`) so a failure can actually be inspected for "already locked"
/// — and, as a side benefit, callers now get restic's real error text instead of a generic
/// "Prune failed".
async fn run_one_prune_attempt(
    restic_path: &str,
    full: &FullRepository,
    prune_handle: &PruneHandle,
) -> Result<PruneAttempt, String> {
    use std::io::{BufReader, Read};

    let mut cmd = std::process::Command::new(restic_path);
    cmd.arg("prune").env("RESTIC_REPOSITORY", &full.path);
    apply_repo_password(&mut cmd, &full.password);
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
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

    {
        let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }

    let status = loop {
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            // A concurrent cancel_prune's own kill() may have raced the child being stored
            // above and seen `None` in the guard (no-op'd) if cancellation landed between
            // spawn() returning and this function's own store completing. Without our own kill
            // attempt here, the guard-clear right below would silently DROP a still-live
            // std::process::Child — Child::drop() does not kill the OS process — orphaning a
            // `restic prune` that keeps running (and keeps holding the repo's exclusive lock)
            // while the UI already reports this as cancelled. Killing an already-exited/already
            // -killed child is a harmless no-op, so it's safe to always attempt this here.
            if let Ok(mut guard) = prune_handle.child.lock() {
                if let Some(ref mut c) = *guard {
                    let _ = c.kill();
                }
            }
            // kill() only sends the signal — it doesn't wait for the OS to tear the process
            // down, and dropping a `Child` handle (the guard-clear right below) does NOT reap
            // it either. Without this, the killed process lingers as a zombie that Rust never
            // waits on again. While it's a zombie its PID is still "alive" as far as a
            // same-host liveness check is concerned, which can fool restic's own stale-lock
            // detection into believing the lock's owning process is still running — so the
            // `unlock` call below silently no-ops and the repo stays exclusively locked for
            // the next prune attempt. Poll (non-blockingly) until the process is actually
            // reaped before proceeding.
            loop {
                let reaped = {
                    let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
                    match *guard {
                        Some(ref mut c) => c.try_wait().map_err(|e| e.to_string())?.is_some(),
                        None => true,
                    }
                };
                if reaped {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            break None;
        }
        let maybe_status = {
            let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
            if let Some(ref mut c) = *guard {
                c.try_wait().map_err(|e| e.to_string())?
            } else {
                break None;
            }
        };
        if let Some(s) = maybe_status {
            break Some(s);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    };

    {
        let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
        *guard = None;
    }

    let captured_stderr = stderr_thread.join().unwrap_or_default();

    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        let unlock_repo = FullRepository { path: full.path.clone(), password: full.password.clone() };
        let _ = run_restic_blocking(unlock_repo, vec!["unlock".to_string()], restic_path.to_string()).await;
        return Ok(PruneAttempt::Cancelled);
    }

    let status = status.ok_or_else(|| "Prune ended unexpectedly".to_string())?;
    if status.success() {
        Ok(PruneAttempt::Success)
    } else {
        let msg = captured_stderr.trim();
        Ok(PruneAttempt::Failed(if msg.is_empty() { "Prune failed".to_string() } else { msg.to_string() }))
    }
}

#[tauri::command]
pub async fn prune_all_repos(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    prune_handle: State<'_, PruneHandle>,
    repo_locks: State<'_, RepoLocks>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Serializes prune_repo/prune_all_repos — they previously shared this handle with no
    // serialization, so a concurrent second run could clobber the first run's
    // `child`/`cancelled` state (a second Stop could kill the wrong process, or vice versa).
    if prune_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A prune is already in progress".to_string());
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&prune_handle.busy);

    prune_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);

    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Prune,
        // No single repoId for a multi-repo prune — left empty, matching the
        // "done" prune:progress event's existing empty-repoId convention.
        String::new(),
        None,
        TaskOrigin::Manual,
        Some(prune_handle.current_task.clone()),
    );
    let task_progress = task_ctx.progress_emitter();

    // Everything fallible below is captured into `result` (via `break 'body` /
    // explicit match instead of `?`/`return`) rather than exiting the fn directly,
    // so the task_ctx terminal call below always runs exactly once, matching the
    // right phase (Finished/Cancelled/Failed) for every exit path.
    let result: Result<(), String> = 'body: {
        let key = match master_key.get() {
            Ok(k) => k,
            Err(e) => break 'body Err(e),
        };
        let repos = match db.list_repos() {
            Ok(r) => r,
            Err(e) => break 'body Err(e),
        };
        let total = repos.len();
        let restic_path = super::get_restic_path(&db);

        for (i, repo) in repos.iter().enumerate() {
            if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                break 'body Err("Cancelled".to_string());
            }

            task_progress.emit(TaskProgress {
                items_done: Some(i as u64),
                items_total: Some(total as u64),
                label: Some(repo.name.clone()),
                repo_id: Some(repo.id.clone()),
                ..Default::default()
            });

            let full = match db.get_full_repo(&repo.id, &key) {
                Ok(r) => r,
                Err(e) => break 'body Err(e),
            };

            // `prune` takes restic's exclusive lock — wait for this repo to go idle first
            // (see CLAUDE.md's Concurrency section / repo_locks.rs). Scoped to this loop
            // iteration: dropped (releasing the exclusive claim) before the next repo.
            let _wg = repo_locks.write(&full.path).await;

            // The wait above can take a while if the repo was genuinely busy — re-check
            // cancellation before spawning, otherwise a Stop click during the wait can't be
            // caught by cancel_prune (child was still None) and this repo's restic process
            // would be orphaned, unkillable, running to completion in the background.
            if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                break 'body Err("Cancelled".to_string());
            }

            // Retry on a genuine EXTERNAL lock collision (a different machine/tool's restic
            // process — RepoLocks above only coordinates this app's own operations).
            let mut outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                Ok(o) => o,
                Err(e) => break 'body Err(e),
            };
            for _ in 0..2 {
                match &outcome {
                    PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                            break 'body Err("Cancelled".to_string());
                        }
                        outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                            Ok(o) => o,
                            Err(e) => break 'body Err(e),
                        };
                    }
                    _ => break,
                }
            }

            match outcome {
                PruneAttempt::Success => {}
                PruneAttempt::Cancelled => break 'body Err("Cancelled".to_string()),
                PruneAttempt::Failed(msg) => break 'body Err(format!("Prune failed for '{}': {}", repo.name, msg)),
            }
        }

        Ok(())
    };

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[tauri::command]
pub async fn prune_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    prune_handle: State<'_, PruneHandle>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    if prune_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A prune is already in progress".to_string());
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&prune_handle.busy);

    prune_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);

    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Prune,
        repo_id.clone(),
        None,
        TaskOrigin::Manual,
        Some(prune_handle.current_task.clone()),
    );

    let result: Result<(), String> = 'body: {
        let key = match master_key.get() {
            Ok(k) => k,
            Err(e) => break 'body Err(e),
        };
        let full = match db.get_full_repo(&repo_id, &key) {
            Ok(r) => r,
            Err(e) => break 'body Err(e),
        };
        let restic_path = super::get_restic_path(&db);

        // `prune` takes restic's exclusive lock — wait for the repo to go idle first (see
        // CLAUDE.md's Concurrency section / repo_locks.rs).
        let _wg = repo_locks.write(&full.path).await;

        // The wait above can take a while if the repo was genuinely busy. If Stop was clicked
        // during that wait, prune_handle.child was still None at that moment, so cancel_prune's
        // kill() was a no-op — bail out now, before spawning, so we never orphan an unkillable
        // restic process. `_wg` and `_busy` both drop automatically on this early return,
        // releasing the exclusive claim and the busy flag exactly like every other early-return
        // path in this function already does.
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            break 'body Err("Cancelled".to_string());
        }

        // Run the prune, retrying up to 2 additional times if it collides with a *different*
        // process's or machine's genuine restic lock — RepoLocks above only coordinates this app's
        // own operations, not an external restic/Backrest/other-computer process (see CLAUDE.md's
        // Concurrency section). Matches apply_retention's retry pattern.
        let mut outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
            Ok(o) => o,
            Err(e) => break 'body Err(e),
        };
        for _ in 0..2 {
            match &outcome {
                PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    // A cancel during the inter-retry sleep must stop us from spawning another
                    // attempt — same reasoning as the cancellation check above.
                    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                        break 'body Err("Cancelled".to_string());
                    }
                    outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                        Ok(o) => o,
                        Err(e) => break 'body Err(e),
                    };
                }
                _ => break,
            }
        }

        match outcome {
            PruneAttempt::Success => Ok(()),
            PruneAttempt::Cancelled => Err("Cancelled".to_string()),
            PruneAttempt::Failed(msg) => Err(msg),
        }
    };

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[tauri::command]
pub fn get_tray_enabled(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("tray_enabled", "false")? == "true")
}

#[tauri::command]
pub fn set_tray_enabled(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("tray_enabled", if value { "true" } else { "false" })
}

#[tauri::command]
pub fn get_tray_warning() -> &'static str {
    #[cfg(target_os = "linux")]
    return "System tray support on Linux depends on your desktop environment. It works on KDE and XFCE, but GNOME requires the AppIndicator extension. If the tray icon does not appear after enabling, the app will continue running as a background process — relaunch it to restore the window.";
    #[cfg(not(target_os = "linux"))]
    return "";
}

#[tauri::command]
pub fn get_remote_auto_refresh(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("remote_auto_refresh", "false")? == "true")
}

#[tauri::command]
pub fn set_remote_auto_refresh(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("remote_auto_refresh", if value { "true" } else { "false" })
}

#[tauri::command]
pub fn get_auto_indexing(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("auto_indexing", "false")? == "true")
}

#[tauri::command]
pub fn set_auto_indexing(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("auto_indexing", if value { "true" } else { "false" })
}

#[tauri::command]
pub async fn cancel_prune(app: tauri::AppHandle, prune_handle: State<'_, PruneHandle>) -> Result<(), String> {
    emit_cancelling(&app, &prune_handle.current_task);
    prune_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut guard) = prune_handle.child.lock() {
        if let Some(ref mut child) = *guard {
            let _ = child.kill();
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct FullDiskAccessStatus {
    pub supported: bool,
    pub granted: bool,
}

#[tauri::command]
pub fn check_full_disk_access() -> Result<FullDiskAccessStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return Ok(FullDiskAccessStatus { supported: true, granted: false });
        }
        let db_path = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
        match std::fs::File::open(&db_path) {
            Ok(_) => Ok(FullDiskAccessStatus { supported: true, granted: true }),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Ok(FullDiskAccessStatus { supported: true, granted: false })
            }
            Err(_) => Ok(FullDiskAccessStatus { supported: true, granted: false }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    Ok(FullDiskAccessStatus { supported: false, granted: false })
}

#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_from_repo_password, apply_repo_password, last_nonblank_line, parse_stats_json,
        validate_init_password, validate_restic_path,
    };

    // ── validate_init_password ─────────────────────────────────────────────

    #[test]
    fn validate_init_password_rejects_empty() {
        assert_eq!(
            validate_init_password("").unwrap_err(),
            "A password is required to create a repository."
        );
    }

    #[test]
    fn validate_init_password_accepts_non_empty() {
        assert!(validate_init_password("hunter2").is_ok());
    }

    // ── apply_repo_password / apply_from_repo_password ─────────────────────

    #[test]
    fn apply_repo_password_sets_env_for_non_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_repo_password(&mut cmd, "hunter2");
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
        assert!(!cmd.get_args().any(|a| a == "--insecure-no-password"));
    }

    #[test]
    fn apply_repo_password_sets_flag_for_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_repo_password(&mut cmd, "");
        assert!(cmd.get_args().any(|a| a == "--insecure-no-password"));
        assert!(!cmd.get_envs().any(|(k, _)| k == "RESTIC_PASSWORD"));
    }

    #[test]
    fn apply_from_repo_password_sets_env_for_non_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_from_repo_password(&mut cmd, "hunter2");
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_FROM_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
        assert!(!cmd.get_args().any(|a| a == "--from-insecure-no-password"));
    }

    #[test]
    fn apply_from_repo_password_sets_flag_for_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_from_repo_password(&mut cmd, "");
        assert!(cmd.get_args().any(|a| a == "--from-insecure-no-password"));
        assert!(!cmd.get_envs().any(|(k, _)| k == "RESTIC_FROM_PASSWORD"));
    }

    // ── last_nonblank_line / parse_stats_json ──────────────────────────────

    #[test]
    fn last_nonblank_line_finds_single_line() {
        assert_eq!(last_nonblank_line(r#"{"total_size":1}"#), Some(r#"{"total_size":1}"#));
    }

    #[test]
    fn last_nonblank_line_skips_trailing_blank_lines() {
        let stdout = "{\"total_size\":1}\n\n   \n";
        assert_eq!(last_nonblank_line(stdout), Some(r#"{"total_size":1}"#));
    }

    #[test]
    fn last_nonblank_line_picks_last_of_multiple_json_lines() {
        // restic can emit progress/status lines before the final summary line.
        let stdout = "{\"message_type\":\"status\"}\n{\"total_size\":42,\"total_file_count\":3,\"snapshots_count\":1}\n";
        assert_eq!(
            last_nonblank_line(stdout),
            Some(r#"{"total_size":42,"total_file_count":3,"snapshots_count":1}"#)
        );
    }

    #[test]
    fn last_nonblank_line_all_blank_returns_none() {
        assert_eq!(last_nonblank_line("\n  \n\t\n"), None);
        assert_eq!(last_nonblank_line(""), None);
    }

    #[test]
    fn parse_stats_json_well_formed() {
        let stdout = r#"{"total_size":100,"total_file_count":10,"snapshots_count":2}"#;
        let stats = parse_stats_json(stdout).unwrap();
        assert_eq!(stats.total_size, 100);
        assert_eq!(stats.total_file_count, 10);
        assert_eq!(stats.snapshots_count, 2);
    }

    #[test]
    fn parse_stats_json_missing_fields_default_to_zero() {
        let stats = parse_stats_json(r#"{"total_size":5}"#).unwrap();
        assert_eq!(stats.total_size, 5);
        assert_eq!(stats.total_file_count, 0);
        assert_eq!(stats.snapshots_count, 0);
    }

    #[test]
    fn parse_stats_json_empty_stdout_is_error() {
        let err = parse_stats_json("").unwrap_err();
        assert!(err.contains("No output"), "unexpected error: {err}");
    }

    #[test]
    fn parse_stats_json_malformed_json_is_error() {
        assert!(parse_stats_json("not json").is_err());
    }

    // ── validate_restic_path ────────────────────────────────────────────────

    #[test]
    fn validate_restic_path_rejects_empty() {
        assert!(validate_restic_path("").is_err());
    }

    #[test]
    fn validate_restic_path_accepts_bare_command_name() {
        // "restic" alone (no path separator) is never checked against the filesystem —
        // it's resolved against $PATH at call time, not by this validator.
        assert!(validate_restic_path("restic").is_ok());
    }

    #[test]
    fn validate_restic_path_accepts_existing_absolute_file() {
        // The current test binary is guaranteed to exist at an absolute path.
        let exe = std::env::current_exe().unwrap();
        assert!(validate_restic_path(exe.to_str().unwrap()).is_ok());
    }

    #[test]
    fn validate_restic_path_rejects_nonexistent_absolute_file() {
        let err = validate_restic_path("/nonexistent/xyz/restic").unwrap_err();
        assert!(err.contains("No file found"), "unexpected error: {err}");
    }

    #[test]
    fn validate_restic_path_rejects_nonexistent_windows_style_paths() {
        assert!(validate_restic_path(r"C:\nonexistent\restic.exe").is_err());
        assert!(validate_restic_path(r"\nonexistent\restic").is_err());
    }
}
