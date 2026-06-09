use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use super::backup_plan::RetentionPolicy;
use super::cache::SnapshotCache;
use super::repo::{run_restic_with_path, Repository};

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
    cache: State<'_, SnapshotCache>,
    repo: Repository,
) -> Result<Vec<Snapshot>, String> {
    if let Some(json) = cache.get_snapshots(&repo.id)? {
        if let Ok(cached) = serde_json::from_str::<Vec<Snapshot>>(&json) {
            return Ok(cached);
        }
    }
    Ok(vec![])
}

#[tauri::command]
pub async fn refresh_snapshots(
    app: AppHandle,
    cache: State<'_, SnapshotCache>,
    repo: Repository,
) -> Result<Vec<Snapshot>, String> {
    let restic_path = super::get_restic_path(&app);
    let stdout = run_restic_with_path(&repo, vec!["snapshots", "--json"], &restic_path)?;
    let _ = cache.set_snapshots(&repo.id, &stdout);
    let snapshots: Vec<Snapshot> = serde_json::from_str(&stdout).map_err(|e| e.to_string())?;
    Ok(snapshots)
}

#[tauri::command]
pub async fn delete_snapshot(
    app: AppHandle,
    cache: State<'_, SnapshotCache>,
    repo: Repository,
    snapshot_id: String,
    prune: bool,
) -> Result<(), String> {
    let restic_path = super::get_restic_path(&app);
    let mut args = vec!["forget", snapshot_id.as_str()];
    if prune {
        args.push("--prune");
    }
    run_restic_with_path(&repo, args, &restic_path)?;
    let _ = cache.evict(&snapshot_id);
    let _ = cache.evict_snapshots(&repo.id);
    Ok(())
}

#[tauri::command]
pub async fn tag_snapshot(
    app: AppHandle,
    repo: Repository,
    snapshot_id: String,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
) -> Result<(), String> {
    let restic_path = super::get_restic_path(&app);

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

#[tauri::command]
pub async fn run_backup(
    app: AppHandle,
    cache: State<'_, SnapshotCache>,
    repo: Repository,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
) -> Result<String, String> {
    let restic_path = super::get_restic_path(&app);

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

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_restic_with_path(&repo, args_refs, &restic_path);
    if result.is_ok() {
        let _ = cache.evict_snapshots(&repo.id);
    }
    result
}

#[tauri::command]
pub async fn forget_by_plan(
    app: AppHandle,
    cache: State<'_, SnapshotCache>,
    repo: Repository,
    tags: Vec<String>,
    paths: Vec<String>,
    retention: RetentionPolicy,
) -> Result<String, String> {
    let restic_path = super::get_restic_path(&app);

    let mut args: Vec<String> = vec!["forget".to_string(), "--prune".to_string(), "--json".to_string()];

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
        let _ = cache.evict_snapshots(&repo.id);
    }
    result
}
