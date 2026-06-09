use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use super::repo::{run_restic_with_path, Repository};

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

#[tauri::command]
pub async fn list_files(
    app: AppHandle,
    repo: Repository,
    snapshot_id: String,
    path: Option<String>,
) -> Result<Vec<FileEntry>, String> {
    let restic_path = super::get_restic_path(&app);

    let mut args = vec!["ls", "--json", snapshot_id.as_str()];
    let path_str;
    if let Some(ref p) = path {
        path_str = p.clone();
        args.push(&path_str);
    }

    let stdout = run_restic_with_path(&repo, args, &restic_path)?;

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
    let restic_path = super::get_restic_path(&app);
    run_restic_with_path(
        &repo,
        vec!["restore", &snapshot_id, "--include", &include_path, "--target", &target_dir],
        &restic_path,
    ).map(|_| ())
}
