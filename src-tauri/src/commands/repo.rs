use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, FullRepository, MasterKey, PruneHandle, Repository};
use super::crypto;
use super::repo_locks::RepoLocks;
use super::NoConsole;

#[derive(Debug, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
}

pub fn run_restic_with_path(
    repo: &FullRepository,
    args: Vec<&str>,
    restic_path: &str,
) -> Result<String, String> {
    let output = std::process::Command::new(restic_path)
        .args(args)
        .env("RESTIC_REPOSITORY", &repo.path)
        .env("RESTIC_PASSWORD", &repo.password)
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
    db: State<'_, AppDb>,
    path: String,
    password: String,
) -> Result<(), String> {
    let restic_path = super::get_restic_path(&db);
    let dummy = FullRepository { path, password };
    run_restic_blocking(dummy, vec!["snapshots".into(), "--json".into()], restic_path)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn get_repo_stats(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<ResticStats, String> {
    if let Ok(Some((total_size, total_file_count, snapshots_count))) = db.get_stats(&repo_id) {
        return Ok(ResticStats { total_size, total_file_count, snapshots_count });
    }
    fetch_and_cache_stats(&db, &master_key, &repo_locks, &repo_id).await
}

#[tauri::command]
pub async fn refresh_repo_stats(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<ResticStats, String> {
    let _ = db.evict_stats(&repo_id);
    fetch_and_cache_stats(&db, &master_key, &repo_locks, &repo_id).await
}

async fn fetch_and_cache_stats(
    db: &AppDb,
    master_key: &MasterKey,
    repo_locks: &RepoLocks,
    repo_id: &str,
) -> Result<ResticStats, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(repo_id, &key)?;
    let restic_path = super::get_restic_path(db);
    let _rg = repo_locks.read(&repo.path);
    let stdout = run_restic_blocking(repo, vec!["stats".into(), "--json".into()], restic_path).await?;
    let last_line = stdout.lines().filter(|l| !l.trim().is_empty()).last()
        .ok_or_else(|| "No output from restic stats".to_string())?;
    let v: serde_json::Value = serde_json::from_str(last_line).map_err(|e| e.to_string())?;
    let stats = ResticStats {
        total_size: v["total_size"].as_u64().unwrap_or(0),
        total_file_count: v["total_file_count"].as_u64().unwrap_or(0),
        snapshots_count: v["snapshots_count"].as_u64().unwrap_or(0),
    };
    let _ = db.set_stats(repo_id, stats.total_size, stats.total_file_count, stats.snapshots_count);
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
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<CheckResult, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    // `check` is a shared-lock read — register as a reader, held across the
    // spawn_blocking below for the whole child-process lifetime.
    let _rg = repo_locks.read(&repo.path);

    let (output, duration_seconds) = tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let output = std::process::Command::new(&restic_path)
            .args(["check", "--json"])
            .env("RESTIC_REPOSITORY", &repo.path)
            .env("RESTIC_PASSWORD", &repo.password)
            .stdin(std::process::Stdio::null())
            .no_console()
            .augment_path()
            .output()
            .map_err(|e| format!("Failed to run restic: {e}"))?;
        Ok::<_, String>((output, started.elapsed().as_secs_f64()))
    })
    .await
    .map_err(|e| e.to_string())??;

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
    if trimmed.is_empty() {
        return Err("Restic path must not be empty".to_string());
    }
    // If the value looks like an absolute path, verify the file exists.
    if trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.contains(":\\") {
        if !std::path::Path::new(trimmed).is_file() {
            return Err(format!("No file found at '{trimmed}'"));
        }
    }
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PruneProgress {
    current: usize,
    total: usize,
    repo_name: String,
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

    let mut child = std::process::Command::new(restic_path)
        .arg("prune")
        .env("RESTIC_REPOSITORY", &full.path)
        .env("RESTIC_PASSWORD", &full.password)
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

    let key = master_key.get()?;
    let repos = db.list_repos()?;
    let total = repos.len();
    let restic_path = super::get_restic_path(&db);

    for (i, repo) in repos.iter().enumerate() {
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }

        let _ = app.emit("prune:progress", PruneProgress {
            current: i,
            total,
            repo_name: repo.name.clone(),
        });

        let full = db.get_full_repo(&repo.id, &key)?;

        // `prune` takes restic's exclusive lock — wait for this repo to go idle first
        // (see CLAUDE.md's Concurrency section / repo_locks.rs). Scoped to this loop
        // iteration: dropped (releasing the exclusive claim) before the next repo.
        let _wg = repo_locks.write(&full.path).await;

        // The wait above can take a while if the repo was genuinely busy — re-check
        // cancellation before spawning, otherwise a Stop click during the wait can't be
        // caught by cancel_prune (child was still None) and this repo's restic process
        // would be orphaned, unkillable, running to completion in the background.
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }

        // Retry on a genuine EXTERNAL lock collision (a different machine/tool's restic
        // process — RepoLocks above only coordinates this app's own operations).
        let mut outcome = run_one_prune_attempt(&restic_path, &full, &prune_handle).await?;
        for _ in 0..2 {
            match &outcome {
                PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err("Cancelled".to_string());
                    }
                    outcome = run_one_prune_attempt(&restic_path, &full, &prune_handle).await?;
                }
                _ => break,
            }
        }

        match outcome {
            PruneAttempt::Success => {}
            PruneAttempt::Cancelled => return Err("Cancelled".to_string()),
            PruneAttempt::Failed(msg) => return Err(format!("Prune failed for '{}': {}", repo.name, msg)),
        }
    }

    let _ = app.emit("prune:progress", PruneProgress {
        current: total,
        total,
        repo_name: String::new(),
    });

    Ok(())
}

#[tauri::command]
pub async fn prune_repo(
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

    let key = master_key.get()?;
    let full = db.get_full_repo(&repo_id, &key)?;
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
        return Err("Cancelled".to_string());
    }

    // Run the prune, retrying up to 2 additional times if it collides with a *different*
    // process's or machine's genuine restic lock — RepoLocks above only coordinates this app's
    // own operations, not an external restic/Backrest/other-computer process (see CLAUDE.md's
    // Concurrency section). Matches apply_retention's retry pattern.
    let mut outcome = run_one_prune_attempt(&restic_path, &full, &prune_handle).await?;
    for _ in 0..2 {
        match &outcome {
            PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                // A cancel during the inter-retry sleep must stop us from spawning another
                // attempt — same reasoning as the cancellation check above.
                if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("Cancelled".to_string());
                }
                outcome = run_one_prune_attempt(&restic_path, &full, &prune_handle).await?;
            }
            _ => break,
        }
    }

    match outcome {
        PruneAttempt::Success => Ok(()),
        PruneAttempt::Cancelled => Err("Cancelled".to_string()),
        PruneAttempt::Failed(msg) => Err(msg),
    }
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
pub async fn cancel_prune(prune_handle: State<'_, PruneHandle>) -> Result<(), String> {
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
            Ok(_) => return Ok(FullDiskAccessStatus { supported: true, granted: true }),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(FullDiskAccessStatus { supported: true, granted: false });
            }
            Err(_) => return Ok(FullDiskAccessStatus { supported: true, granted: false }),
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
