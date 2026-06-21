use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, FullRepository, MasterKey, PruneHandle, Repository};
use super::crypto;
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
    let output = std::process::Command::new(&restic_path)
        .args(["init"])
        .env("RESTIC_REPOSITORY", &dummy.path)
        .env("RESTIC_PASSWORD", &dummy.password)
        .no_console()
        .augment_path()
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }

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
    run_restic_with_path(&dummy, vec!["snapshots", "--json"], &restic_path).map(|_| ())
}

#[tauri::command]
pub async fn get_repo_stats(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<ResticStats, String> {
    if let Ok(Some((total_size, total_file_count, snapshots_count))) = db.get_stats(&repo_id) {
        return Ok(ResticStats { total_size, total_file_count, snapshots_count });
    }
    fetch_and_cache_stats(&db, &master_key, &repo_id)
}

#[tauri::command]
pub async fn refresh_repo_stats(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<ResticStats, String> {
    let _ = db.evict_stats(&repo_id);
    fetch_and_cache_stats(&db, &master_key, &repo_id)
}

fn fetch_and_cache_stats(
    db: &AppDb,
    master_key: &MasterKey,
    repo_id: &str,
) -> Result<ResticStats, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(repo_id, &key)?;
    let restic_path = super::get_restic_path(db);
    let stdout = run_restic_with_path(&repo, vec!["stats", "--json"], &restic_path)?;
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
    repo_id: String,
) -> Result<CheckResult, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let started = std::time::Instant::now();
    let output = std::process::Command::new(&restic_path)
        .args(["check", "--json"])
        .env("RESTIC_REPOSITORY", &repo.path)
        .env("RESTIC_PASSWORD", &repo.password)
        .no_console()
        .augment_path()
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    let duration_seconds = started.elapsed().as_secs_f64();

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

#[tauri::command]
pub async fn prune_all_repos(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    prune_handle: State<'_, PruneHandle>,
) -> Result<(), String> {
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
        // Stash credentials so we can run `restic unlock` after a cancel-kill.
        let path_for_unlock = full.path.clone();
        let pass_for_unlock = full.password.clone();
        let restic_path_for_unlock = restic_path.clone();

        let child = std::process::Command::new(&restic_path)
            .arg("prune")
            .env("RESTIC_REPOSITORY", &full.path)
            .env("RESTIC_PASSWORD", &full.password)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .no_console()
            .augment_path()
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        // Store child then immediately release the lock so cancel_prune can always acquire it.
        {
            let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
            *guard = Some(child);
        }

        // Poll for completion with brief lock windows so cancel_prune is never blocked.
        let status = loop {
            if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                break None;
            }
            let maybe_status = {
                let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
                if let Some(ref mut c) = *guard {
                    c.try_wait().map_err(|e| e.to_string())?
                } else {
                    break None; // killed by cancel_prune
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

        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            let unlock_repo = FullRepository { path: path_for_unlock, password: pass_for_unlock };
            let _ = run_restic_with_path(&unlock_repo, vec!["unlock"], &restic_path_for_unlock);
            return Err("Cancelled".to_string());
        }

        let status = status.unwrap();
        if !status.success() {
            return Err(format!("Prune failed for '{}'", repo.name));
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
    repo_id: String,
) -> Result<(), String> {
    prune_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);

    let key = master_key.get()?;
    let full = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let path_for_unlock = full.path.clone();
    let pass_for_unlock = full.password.clone();
    let restic_path_for_unlock = restic_path.clone();

    let child = std::process::Command::new(&restic_path)
        .arg("prune")
        .env("RESTIC_REPOSITORY", &full.path)
        .env("RESTIC_PASSWORD", &full.password)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .no_console()
        .augment_path()
        .spawn()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    {
        let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }

    let status = loop {
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
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

    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        let unlock_repo = FullRepository { path: path_for_unlock, password: pass_for_unlock };
        let _ = run_restic_with_path(&unlock_repo, vec!["unlock"], &restic_path_for_unlock);
        return Err("Cancelled".to_string());
    }

    let status = status.unwrap();
    if !status.success() {
        return Err("Prune failed".to_string());
    }

    Ok(())
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
    return "System tray support on Linux depends on your desktop environment. It works on KDE and XFCE, but GNOME requires the AppIndicator extension. If the tray icon does not appear after enabling, the window may become unreachable until the app is relaunched.";
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
pub async fn cancel_prune(prune_handle: State<'_, PruneHandle>) -> Result<(), String> {
    prune_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut guard) = prune_handle.child.lock() {
        if let Some(ref mut child) = *guard {
            let _ = child.kill();
        }
    }
    Ok(())
}
