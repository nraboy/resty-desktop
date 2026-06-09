use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreProgress {
    pub percent_done: f64,
    pub files_restored: u64,
    pub total_files: u64,
    pub bytes_restored: u64,
    pub total_bytes: u64,
    pub seconds_elapsed: u64,
}

#[tauri::command]
pub async fn restore_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    snapshot_id: String,
    target_dir: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let repo_path = repo.path.clone();
    let repo_password = repo.password.clone();

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut child = std::process::Command::new(&restic_path)
            .args(["restore", &snapshot_id, "--target", &target_dir, "--json"])
            .env("RESTIC_REPOSITORY", &repo_path)
            .env("RESTIC_PASSWORD", &repo_password)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        let stderr = child.stderr.take().unwrap();
        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        let stdout = child.stdout.take().unwrap();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v["message_type"].as_str() == Some("status") {
                    let progress = RestoreProgress {
                        percent_done: v["percent_done"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0),
                        files_restored: v["files_restored"].as_u64().unwrap_or(0),
                        total_files: v["total_files"].as_u64().unwrap_or(0),
                        bytes_restored: v["bytes_restored"].as_u64().unwrap_or(0),
                        total_bytes: v["total_bytes"].as_u64().unwrap_or(0),
                        seconds_elapsed: v["seconds_elapsed"].as_u64().unwrap_or(0),
                    };
                    let _ = app.emit("restore:progress", &progress);
                }
            }
        }

        let status = child.wait().map_err(|e| e.to_string())?;
        let stderr_str = stderr_thread.join().unwrap_or_default();

        if status.success() {
            Ok(())
        } else {
            let msg = stderr_str.trim();
            Err(if msg.is_empty() { "restic restore failed".to_string() } else { msg.to_string() })
        }
    })
    .await
    .map_err(|e| e.to_string())?
}
