use serde::{Deserialize, Serialize};
use tauri::State;

use super::cache::{AppDb, FullRepository, MasterKey, Repository};
use super::crypto;

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
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
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
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
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
    let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
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
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    let duration_seconds = started.elapsed().as_secs_f64();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut errors: Vec<String> = Vec::new();

    for line in stdout.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v["message_type"].as_str() == Some("error") {
                if let Some(msg) = v["error"]["message"].as_str() {
                    errors.push(msg.to_string());
                }
            }
        }
    }

    if !output.status.success() && errors.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            errors.push(stderr);
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
    db.set_setting("restic_path", &path)
}

#[tauri::command]
pub fn get_restic_version(db: State<'_, AppDb>) -> Result<String, String> {
    let restic_path = super::get_restic_path(&db);
    let output = std::process::Command::new(&restic_path)
        .arg("version")
        .output()
        .map_err(|_| format!("restic not found at '{restic_path}'"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}
