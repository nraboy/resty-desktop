use serde::{Deserialize, Serialize};
use tauri::State;

use super::cache::{AppDb, MasterKey};
use super::repo::run_restic_with_path;

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
    repo_id: String,
    snapshot_id: String,
    path: Option<String>,
) -> Result<Vec<FileEntry>, String> {
    if let Some(cached) = db.get(&snapshot_id, path.as_deref())? {
        return Ok(cached);
    }

    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let mut args = vec!["ls", "--json", snapshot_id.as_str()];
    let path_str;
    if let Some(ref p) = path {
        path_str = p.clone();
        args.push(&path_str);
    }

    let stdout = run_restic_with_path(&repo, args, &restic_path)?;

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
pub async fn restore_path(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    snapshot_id: String,
    include_path: String,
    target_dir: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    run_restic_with_path(
        &repo,
        vec![
            "restore",
            &snapshot_id,
            "--include",
            &include_path,
            "--target",
            &target_dir,
        ],
        &restic_path,
    )
    .map(|_| ())
}
