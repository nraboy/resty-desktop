use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use super::cache::{AppDb, BackupHandle, CopyHandle, FullRepository, MasterKey, MirrorHandle, RetentionPolicy};
use super::repo::run_restic_with_path;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProgress {
    pub percent_done: f64,
    pub files_done: u64,
    pub total_files: u64,
    pub bytes_done: u64,
    pub total_bytes: u64,
    pub seconds_elapsed: u64,
    pub seconds_remaining: Option<u64>,
    pub current_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub short_id: String,
    pub time: String,
    pub hostname: String,
    pub username: Option<String>,
    pub paths: Vec<String>,
    pub tags: Option<Vec<String>>,
}

#[tauri::command]
pub async fn list_snapshots(
    db: State<'_, AppDb>,
    repo_id: String,
) -> Result<Vec<Snapshot>, String> {
    if let Some(json) = db.get_snapshots(&repo_id)? {
        if let Ok(cached) = serde_json::from_str::<Vec<Snapshot>>(&json) {
            return Ok(cached);
        }
    }
    Ok(vec![])
}

#[tauri::command]
pub async fn refresh_snapshots(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<Vec<Snapshot>, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let stdout = run_restic_with_path(&repo, vec!["snapshots", "--json"], &restic_path)?;
    let _ = db.set_snapshots(&repo_id, &stdout);
    let snapshots: Vec<Snapshot> = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
    Ok(snapshots)
}

#[tauri::command]
pub async fn delete_snapshot(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    snapshot_id: String,
    prune: bool,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let mut args = vec!["forget", snapshot_id.as_str()];
    if prune {
        args.push("--prune");
    }
    run_restic_with_path(&repo, args, &restic_path)?;
    let _ = db.evict(&snapshot_id);
    let _ = db.evict_snapshots(&repo_id);
    let _ = db.evict_stats(&repo_id);
    Ok(())
}

#[tauri::command]
pub async fn tag_snapshot(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    snapshot_id: String,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    if !add_tags.is_empty() {
        let tag_str = add_tags.join(",");
        run_restic_with_path(
            &repo,
            vec!["tag", "--add", &tag_str, &snapshot_id],
            &restic_path,
        )?;
    }
    if !remove_tags.is_empty() {
        let tag_str = remove_tags.join(",");
        run_restic_with_path(
            &repo,
            vec!["tag", "--remove", &tag_str, &snapshot_id],
            &restic_path,
        )?;
    }
    Ok(())
}

pub async fn execute_backup(
    app: &tauri::AppHandle,
    db: &AppDb,
    master_key: &MasterKey,
    backup_handle: &BackupHandle,
    repo_id: &str,
    plan_id: Option<&str>,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
) -> Result<String, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(repo_id, &key)?;
    let restic_path = super::get_restic_path(db);

    let mut args: Vec<String> = vec!["backup".to_string(), "--json".to_string()];
    for tag in &tags {
        args.push("--tag".to_string());
        args.push(tag.clone());
    }
    for pattern in &excludes {
        let trimmed = pattern.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            args.push("--exclude".to_string());
            args.push(trimmed.to_string());
        }
    }
    for path in &paths {
        args.push(path.clone());
    }

    let started = std::time::Instant::now();
    let started_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let app_inner = app.clone();
    let repo_path = repo.path.clone();
    let repo_password = repo.password.clone();
    let repo_path_for_unlock = repo.path.clone();
    let repo_pass_for_unlock = repo.password.clone();
    let restic_path_inner = restic_path.clone();
    let restic_path_for_unlock = restic_path.clone();

    backup_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&backup_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&backup_handle.cancelled);

    let result: Result<String, String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut child = std::process::Command::new(&restic_path_inner)
            .args(&args)
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

        // Store child so cancel_backup can reach it.
        *child_arc.lock().unwrap() = Some(child);

        let reader = BufReader::new(stdout);
        let mut all_lines: Vec<String> = Vec::new();

        for line in reader.lines() {
            if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let line = line.map_err(|e| e.to_string())?;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v["message_type"].as_str() == Some("status") {
                    let progress = BackupProgress {
                        percent_done: v["percent_done"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0),
                        files_done: v["files_done"].as_u64().unwrap_or(0),
                        total_files: v["total_files"].as_u64().unwrap_or(0),
                        bytes_done: v["bytes_done"].as_u64().unwrap_or(0),
                        total_bytes: v["total_bytes"].as_u64().unwrap_or(0),
                        seconds_elapsed: v["seconds_elapsed"].as_u64().unwrap_or(0),
                        seconds_remaining: v["seconds_remaining"].as_u64(),
                        current_files: v["current_files"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|f| f.as_str().map(str::to_string)).collect())
                            .unwrap_or_default(),
                    };
                    let _ = app_inner.emit("backup:progress", &progress);
                }
            }
            all_lines.push(line);
        }

        let status = match child_arc.lock().unwrap().take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };
        let stderr_str = stderr_thread.join().unwrap_or_default();

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }

        if status.success() {
            Ok(all_lines.join("\n"))
        } else {
            let msg = stderr_str.trim();
            Err(if msg.is_empty() { "restic backup failed".to_string() } else { msg.to_string() })
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    if backup_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let repo = FullRepository { path: repo_path_for_unlock, password: repo_pass_for_unlock };
            let _ = run_restic_with_path(&repo, vec!["unlock"], &restic_path_for_unlock);
        });
    }

    let duration = started.elapsed().as_secs_f64();

    use tauri_plugin_notification::NotificationExt;
    use rand::Rng;
    let history_id: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();

    match result {
        Ok(ref stdout) => {
            let _ = db.evict_stats(repo_id);

            let summary = stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .find(|v| v.get("message_type").and_then(|t| t.as_str()) == Some("summary"));

            let snapshot_id = summary.as_ref()
                .and_then(|v| v["snapshot_id"].as_str().map(str::to_string));
            let files_new = summary.as_ref().and_then(|v| v["files_new"].as_u64()).unwrap_or(0);
            let files_changed = summary.as_ref().and_then(|v| v["files_changed"].as_u64()).unwrap_or(0);
            let bytes_added = summary.as_ref().and_then(|v| v["data_added"].as_u64()).unwrap_or(0);

            let appended: Option<String> = snapshot_id.clone().and_then(|id| {
                let new_json = run_restic_with_path(&repo, vec!["snapshots", "--json", &id], &restic_path).ok()?;
                let mut new_snaps: Vec<serde_json::Value> = serde_json::from_str(&new_json).ok()?;
                let existing: Vec<serde_json::Value> = db
                    .get_snapshots(repo_id)
                    .ok()
                    .flatten()
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default();
                new_snaps.extend(existing);
                serde_json::to_string(&new_snaps).ok()
            });

            if let Some(json) = appended {
                let _ = db.set_snapshots(repo_id, &json);
            } else {
                let _ = db.evict_snapshots(repo_id);
            }

            let _ = db.log_backup(
                &history_id, repo_id, plan_id, snapshot_id.as_deref(),
                started_at, duration, files_new, files_changed, bytes_added, None,
            );

            let body = format!(
                "{} new, {} changed · {:.1}s",
                files_new, files_changed, duration
            );
            let _ = app.notification().builder()
                .title("Backup completed")
                .body(&body)
                .show();

            Ok(stdout.clone())
        }
        Err(ref err) => {
            let _ = db.log_backup(
                &history_id, repo_id, plan_id, None,
                started_at, duration, 0, 0, 0, Some(err.as_str()),
            );

            let _ = app.notification().builder()
                .title("Backup failed")
                .body(err)
                .show();

            Err(err.clone())
        }
    }
}

#[tauri::command]
pub async fn run_backup(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    backup_handle: State<'_, BackupHandle>,
    repo_id: String,
    plan_id: Option<String>,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
) -> Result<String, String> {
    execute_backup(&app, &db, &master_key, &backup_handle, &repo_id, plan_id.as_deref(), paths, tags, excludes).await
}

#[tauri::command]
pub async fn cancel_backup(backup_handle: State<'_, BackupHandle>) -> Result<(), String> {
    backup_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut guard) = backup_handle.child.lock() {
        if let Some(ref mut child) = *guard {
            let _ = child.kill();
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn copy_snapshot(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    copy_handle: State<'_, CopyHandle>,
    src_repo_id: String,
    dest_repo_id: String,
    snapshot_id: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let src_repo = db.get_full_repo(&src_repo_id, &key)?;
    let dest_repo = db.get_full_repo(&dest_repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    copy_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&copy_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&copy_handle.cancelled);

    // Stash credentials so we can run `restic unlock` after a cancel-kill.
    let src_path_for_unlock = src_repo.path.clone();
    let src_pass_for_unlock = src_repo.password.clone();
    let dst_path_for_unlock = dest_repo.path.clone();
    let dst_pass_for_unlock = dest_repo.password.clone();
    let restic_path_for_unlock = restic_path.clone();

    let result: Result<(), String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut child = std::process::Command::new(&restic_path)
            .args(["copy", &snapshot_id])
            .env("RESTIC_REPOSITORY", &dest_repo.path)
            .env("RESTIC_PASSWORD", &dest_repo.password)
            .env("RESTIC_FROM_REPOSITORY", &src_repo.path)
            .env("RESTIC_FROM_PASSWORD", &src_repo.password)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        // Store child so cancel_copy can reach it.
        *child_arc.lock().unwrap() = Some(child);

        // Drain stdout so the process isn't blocked on a full pipe buffer.
        for _ in BufReader::new(stdout).lines() {}

        // stdout exhausted (process ended or was killed); take child back to call wait().
        let status = match child_arc.lock().unwrap().take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };

        let stderr_str = stderr_thread.join().unwrap_or_default();

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }

        if status.success() {
            Ok(())
        } else {
            let msg = stderr_str.trim();
            Err(if msg.is_empty() { "restic copy failed".to_string() } else { msg.to_string() })
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    if result.is_ok() {
        let _ = db.evict_snapshots(&dest_repo_id);
        let _ = db.evict_stats(&dest_repo_id);
    } else if copy_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        // The process was killed via SIGKILL and left stale locks on both repos.
        // spawn_blocking already called wait(), so the PIDs are gone — unlock is safe now.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let src = FullRepository { path: src_path_for_unlock, password: src_pass_for_unlock };
            let dst = FullRepository { path: dst_path_for_unlock, password: dst_pass_for_unlock };
            let _ = run_restic_with_path(&src, vec!["unlock"], &restic_path_for_unlock);
            let _ = run_restic_with_path(&dst, vec!["unlock"], &restic_path_for_unlock);
        })
        .await;
    }
    result
}

#[tauri::command]
pub async fn cancel_copy(copy_handle: State<'_, CopyHandle>) -> Result<(), String> {
    copy_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(ref mut child) = *copy_handle.child.lock().map_err(|e| e.to_string())? {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn mirror_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    mirror_handle: State<'_, MirrorHandle>,
    src_repo_id: String,
    dest_repo_id: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let src_repo = db.get_full_repo(&src_repo_id, &key)?;
    let dest_repo = db.get_full_repo(&dest_repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    mirror_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);
    let child_arc = std::sync::Arc::clone(&mirror_handle.child);
    let cancelled_arc = std::sync::Arc::clone(&mirror_handle.cancelled);

    // Stash credentials so we can run `restic unlock` after a cancel-kill.
    let src_path_for_unlock = src_repo.path.clone();
    let src_pass_for_unlock = src_repo.password.clone();
    let dst_path_for_unlock = dest_repo.path.clone();
    let dst_pass_for_unlock = dest_repo.password.clone();
    let restic_path_for_unlock = restic_path.clone();

    let result: Result<(), String> = tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;

        let mut child = std::process::Command::new(&restic_path)
            .args(["copy"])
            .env("RESTIC_REPOSITORY", &dest_repo.path)
            .env("RESTIC_PASSWORD", &dest_repo.password)
            .env("RESTIC_FROM_REPOSITORY", &src_repo.path)
            .env("RESTIC_FROM_PASSWORD", &src_repo.password)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run restic: {e}"))?;

        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let stderr_thread = std::thread::spawn(move || {
            let mut s = String::new();
            BufReader::new(stderr).read_to_string(&mut s).ok();
            s
        });

        *child_arc.lock().unwrap() = Some(child);

        // Drain stdout to avoid blocking the process on a full pipe buffer.
        for _ in BufReader::new(stdout).lines() {}

        let status = match child_arc.lock().unwrap().take() {
            Some(mut c) => c.wait().map_err(|e| e.to_string())?,
            None => return Err("cancelled".to_string()),
        };

        let stderr_str = stderr_thread.join().unwrap_or_default();

        if cancelled_arc.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("cancelled".to_string());
        }

        if status.success() {
            Ok(())
        } else {
            let msg = stderr_str.trim();
            Err(if msg.is_empty() { "restic copy failed".to_string() } else { msg.to_string() })
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    if result.is_ok() {
        let _ = db.evict_snapshots(&dest_repo_id);
        let _ = db.evict_stats(&dest_repo_id);
    } else if mirror_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        // The process was killed via SIGKILL and left stale locks on both repos.
        // spawn_blocking already called wait(), so the PIDs are gone — unlock is safe now.
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let src = FullRepository { path: src_path_for_unlock, password: src_pass_for_unlock };
            let dst = FullRepository { path: dst_path_for_unlock, password: dst_pass_for_unlock };
            let _ = run_restic_with_path(&src, vec!["unlock"], &restic_path_for_unlock);
            let _ = run_restic_with_path(&dst, vec!["unlock"], &restic_path_for_unlock);
        })
        .await;
    }
    result
}

#[tauri::command]
pub async fn cancel_mirror(mirror_handle: State<'_, MirrorHandle>) -> Result<(), String> {
    mirror_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(ref mut child) = *mirror_handle.child.lock().map_err(|e| e.to_string())? {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn unlock_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<(), String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    run_restic_with_path(&repo, vec!["unlock"], &restic_path)?;
    Ok(())
}

#[tauri::command]
pub async fn forget_by_plan(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    tags: Vec<String>,
    paths: Vec<String>,
    retention: RetentionPolicy,
) -> Result<String, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

    let mut args: Vec<String> =
        vec!["forget".to_string(), "--prune".to_string(), "--json".to_string()];

    if !tags.is_empty() {
        args.push("--tag".to_string());
        args.push(tags.join(","));
    } else {
        for path in &paths {
            args.push("--path".to_string());
            args.push(path.clone());
        }
    }

    if let Some(n) = retention.keep_last {
        args.push("--keep-last".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_daily {
        args.push("--keep-daily".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_weekly {
        args.push("--keep-weekly".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_monthly {
        args.push("--keep-monthly".to_string());
        args.push(n.to_string());
    }
    if let Some(n) = retention.keep_yearly {
        args.push("--keep-yearly".to_string());
        args.push(n.to_string());
    }

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_restic_with_path(&repo, args_refs, &restic_path);
    if result.is_ok() {
        let _ = db.evict_stats(&repo_id);
        if let Ok(json) =
            run_restic_with_path(&repo, vec!["snapshots", "--json"], &restic_path)
        {
            let _ = db.set_snapshots(&repo_id, &json);
        } else {
            let _ = db.evict_snapshots(&repo_id);
        }
    }
    result
}
