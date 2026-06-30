use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

use super::cache::{AppDb, MasterKey};
use super::repo::run_restic_with_path;
use super::snapshot::validate_snapshot_id;
use super::NoConsole;

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
    validate_snapshot_id(&snapshot_id)?;
    if let Some(cached) = db.get(&repo_id, &snapshot_id, path.as_deref())? {
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
    strip_leading_path: bool,
) -> Result<(), String> {
    validate_snapshot_id(&snapshot_id)?;
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let restic_result = run_restic_with_path(
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

    if strip_leading_path {
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
    }

    restic_result
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
    validate_snapshot_id(&snapshot_id)?;
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
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shared indexing logic: runs `restic ls --json` for an entire snapshot and
/// bulk-inserts all file entries into `browse_cache_files`. Called by both the
/// manual `index_snapshot` command and the background `cache_warmer`.
pub(crate) fn run_full_index(
    db: &AppDb,
    repo_id: &str,
    repo: &super::cache::FullRepository,
    snapshot_id: &str,
    restic_path: &str,
) -> Result<(), String> {
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

/// Manually trigger full indexing for a snapshot. Fire-and-forget: returns
/// immediately and runs the index in the background. Safe to call on remote
/// repos since the user explicitly requested it.
#[tauri::command]
pub async fn index_snapshot(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
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

    tauri::async_runtime::spawn(async move {
        let repo_path = repo.path.clone();
        let repo_pass = repo.password.clone();
        let snap_id = snapshot_id.clone();
        let repo_id2 = repo_id.clone();
        let rp = restic_path.clone();
        let app2 = app.clone();

        let ok = tauri::async_runtime::spawn_blocking(move || {
            let tmp_repo = super::cache::FullRepository {
                path: repo_path,
                password: repo_pass,
            };
            let db_inner = app.state::<AppDb>();
            run_full_index(&db_inner, &repo_id2, &tmp_repo, &snap_id, &rp).is_ok()
        })
        .await
        .unwrap_or(false);

        if !ok {
            let _ = app2
                .state::<AppDb>()
                .set_browse_status(&repo_id, &snapshot_id, "pending");
        }

        let _ = app2.emit(
            "index:done",
            serde_json::json!({ "snapshotId": snapshot_id, "repoId": repo_id, "success": ok }),
        );
    });

    Ok(true)
}

#[tauri::command]
pub fn search_snapshot_files(
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
    db.search_browse_files(&snapshot_id, &trimmed, 200)
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

}
