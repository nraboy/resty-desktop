use serde::{Deserialize, Serialize};
use tauri::State;

use super::cache::{AppDb, MasterKey, RetentionPolicy};
use super::repo::run_restic_with_path;

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

#[tauri::command]
pub async fn run_backup(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
) -> Result<String, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);

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

    if let Ok(ref stdout) = result {
        let _ = db.evict_stats(&repo_id);

        let new_id = stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|v| v.get("message_type").and_then(|t| t.as_str()) == Some("summary"))
            .and_then(|v| v["snapshot_id"].as_str().map(str::to_string));

        let appended = new_id.and_then(|id| {
            let new_json =
                run_restic_with_path(&repo, vec!["snapshots", "--json", &id], &restic_path).ok()?;
            let mut new_snaps: Vec<serde_json::Value> = serde_json::from_str(&new_json).ok()?;
            let existing: Vec<serde_json::Value> = db
                .get_snapshots(&repo_id)
                .ok()
                .flatten()
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            new_snaps.extend(existing);
            serde_json::to_string(&new_snaps).ok()
        });

        if let Some(json) = appended {
            let _ = db.set_snapshots(&repo_id, &json);
        } else {
            let _ = db.evict_snapshots(&repo_id);
        }
    }
    result
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
