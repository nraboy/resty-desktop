use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub path: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
}

fn run_restic(
    repo: &Repository,
    args: Vec<&str>,
    restic_path: &str,
) -> Result<std::process::Output, String> {
    std::process::Command::new(restic_path)
        .args(args)
        .env("RESTIC_REPOSITORY", &repo.path)
        .env("RESTIC_PASSWORD", &repo.password)
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))
}

pub fn run_restic_with_path(
    repo: &Repository,
    args: Vec<&str>,
    restic_path: &str,
) -> Result<String, String> {
    let output = run_restic(repo, args, restic_path)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub async fn list_repos(app: AppHandle) -> Result<Vec<Repository>, String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    let repos: Vec<Repository> = store
        .get("repos")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(repos)
}

#[tauri::command]
pub async fn add_repo(app: AppHandle, repo: Repository) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    let mut repos: Vec<Repository> = store
        .get("repos")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    repos.push(repo);
    store.set("repos", serde_json::to_value(&repos).map_err(|e| e.to_string())?);
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn remove_repo(app: AppHandle, repo_id: String) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    let mut repos: Vec<Repository> = store
        .get("repos")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    repos.retain(|r| r.id != repo_id);
    store.set("repos", serde_json::to_value(&repos).map_err(|e| e.to_string())?);
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn init_repo(app: AppHandle, repo: Repository) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    let restic_path: String = store
        .get("restic_path")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "restic".to_string());

    let output = std::process::Command::new(&restic_path)
        .args(["init"])
        .env("RESTIC_REPOSITORY", &repo.path)
        .env("RESTIC_PASSWORD", &repo.password)
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub async fn get_repo_stats(app: AppHandle, repo: Repository) -> Result<ResticStats, String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    let restic_path: String = store
        .get("restic_path")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "restic".to_string());

    let stdout = run_restic_with_path(&repo, vec!["stats", "--json"], &restic_path)?;
    let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
    Ok(ResticStats {
        total_size: v["total_size"].as_u64().unwrap_or(0),
        total_file_count: v["total_file_count"].as_u64().unwrap_or(0),
        snapshots_count: v["snapshots_count"].as_u64().unwrap_or(0),
    })
}

#[tauri::command]
pub async fn get_restic_path(app: AppHandle) -> Result<String, String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    Ok(store
        .get("restic_path")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "restic".to_string()))
}

#[tauri::command]
pub async fn set_restic_path(app: AppHandle, path: String) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| e.to_string())?;
    store.set("restic_path", serde_json::Value::String(path));
    store.save().map_err(|e| e.to_string())
}
