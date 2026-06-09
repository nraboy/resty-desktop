use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use super::repo::Repository;

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

fn is_direct_child(entry_path: &str, parent: Option<&str>) -> bool {
    let clean = entry_path.trim_end_matches('/');
    match parent {
        None | Some("") | Some("/") => {
            // Root: path must have exactly one component, e.g. "/Users"
            let mut parts = clean.splitn(3, '/');
            parts.next(); // leading empty string before first '/'
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

fn get_restic_path(app: &AppHandle) -> String {
    app.store("settings.json")
        .ok()
        .and_then(|s| s.get("restic_path"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "restic".to_string())
}

#[tauri::command]
pub async fn list_files(
    app: AppHandle,
    repo: Repository,
    snapshot_id: String,
    path: Option<String>,
) -> Result<Vec<FileEntry>, String> {
    let restic_path = get_restic_path(&app);

    let mut args = vec!["ls", "--json", snapshot_id.as_str()];
    let path_str;
    if let Some(ref p) = path {
        path_str = p.clone();
        args.push(&path_str);
    }

    let output = std::process::Command::new(&restic_path)
        .args(&args)
        .env("RESTIC_REPOSITORY", &repo.path)
        .env("RESTIC_PASSWORD", &repo.password)
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // restic ls --json outputs one JSON object per line; first line is snapshot info
    let mut entries: Vec<FileEntry> = Vec::new();
    for (i, line) in stdout.lines().enumerate() {
        if i == 0 {
            continue; // skip snapshot summary line
        }
        if let Ok(entry) = serde_json::from_str::<FileEntry>(line) {
            if is_direct_child(&entry.path, path.as_deref()) {
                entries.push(entry);
            }
        }
    }
    Ok(entries)
}

#[tauri::command]
pub async fn restore_path(
    app: AppHandle,
    repo: Repository,
    snapshot_id: String,
    include_path: String,
    target_dir: String,
) -> Result<(), String> {
    let restic_path = get_restic_path(&app);

    let output = std::process::Command::new(&restic_path)
        .args([
            "restore",
            &snapshot_id,
            "--include",
            &include_path,
            "--target",
            &target_dir,
        ])
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
