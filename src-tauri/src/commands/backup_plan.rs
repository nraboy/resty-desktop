use tauri::State;

use super::cache::{AppDb, BackupPlan};

#[tauri::command]
pub fn list_backup_plans(db: State<'_, AppDb>) -> Result<Vec<BackupPlan>, String> {
    db.list_backup_plans()
}

/// A plan must back up *something*, and a `--files-from` plan must carry a tag: its
/// snapshots' paths come from the list files, so `forget --path` (the no-tag fallback)
/// can't find them, and a tag is the only handle retention has. Mirrors the editor's checks.
pub(crate) fn validate_plan_sources(plan: &BackupPlan) -> Result<(), String> {
    let has_list = plan.files_from.iter().any(|p| !p.trim().is_empty());
    if plan.paths.is_empty() && !has_list {
        return Err("Add at least one source path or path-list file.".into());
    }
    if has_list && super::snapshot::non_blank_tags(&plan.tags).is_empty() {
        return Err("A plan that uses path-list files needs at least one tag so its \
                    snapshots can be identified."
            .into());
    }
    Ok(())
}

#[tauri::command]
pub fn save_backup_plan(db: State<'_, AppDb>, mut plan: BackupPlan) -> Result<(), String> {
    validate_plan_sources(&plan)?;
    // Reject an out-of-range pack size with a readable error (and fold 0 → None) instead of
    // persisting a value that would make every backup of the plan fail.
    plan.pack_size = super::snapshot::normalize_pack_size(plan.pack_size)?;
    db.save_backup_plan(&plan)
}

#[tauri::command]
pub fn remove_backup_plan(db: State<'_, AppDb>, plan_id: String) -> Result<(), String> {
    db.remove_backup_plan(&plan_id)
}

#[cfg(test)]
mod tests {
    use super::validate_plan_sources;
    use crate::commands::cache::{BackupPlan, RetentionPolicy};

    fn plan() -> BackupPlan {
        BackupPlan {
            id: "p".into(),
            name: "n".into(),
            repo_id: "r".into(),
            paths: vec!["/home".into()],
            tags: vec![],
            excludes: vec![],
            exclude_if_present: vec![],
            exclude_caches: false,
            retention: None,
            limit_upload: None,
            limit_download: None,
            webhooks: vec![],
            pack_size: None,
            exclude_cloud_files: false,
            use_fs_snapshot: false,
            files_from: vec![],
            exclude_files: vec![],
        }
    }

    fn retention() -> Option<RetentionPolicy> {
        Some(RetentionPolicy {
            keep_last: Some(3),
            keep_daily: None,
            keep_weekly: None,
            keep_monthly: None,
            keep_yearly: None,
        })
    }

    #[test]
    fn rejects_plan_with_no_sources() {
        let p = BackupPlan { paths: vec![], ..plan() };
        assert!(validate_plan_sources(&p).is_err());
    }

    #[test]
    fn rejects_whitespace_only_list_file_with_no_paths() {
        let p = BackupPlan { paths: vec![], files_from: vec!["  ".into()], ..plan() };
        assert!(validate_plan_sources(&p).is_err());
    }

    #[test]
    fn accepts_list_file_only_with_a_tag() {
        let p = BackupPlan {
            paths: vec![],
            files_from: vec!["/list.txt".into()],
            tags: vec!["t".into()],
            ..plan()
        };
        assert!(validate_plan_sources(&p).is_ok());
    }

    #[test]
    fn rejects_list_file_with_no_tags_even_without_retention() {
        let p = BackupPlan { paths: vec![], files_from: vec!["/list.txt".into()], ..plan() };
        assert!(validate_plan_sources(&p).is_err());
        // …and a blank tag doesn't count.
        let blank = BackupPlan { tags: vec!["  ".into()], ..p };
        assert!(validate_plan_sources(&blank).is_err());
    }

    #[test]
    fn rejects_list_file_with_retention_and_no_tags() {
        let p = BackupPlan { files_from: vec!["/list.txt".into()], retention: retention(), ..plan() };
        assert!(validate_plan_sources(&p).is_err());
    }

    #[test]
    fn accepts_list_file_with_retention_and_a_tag() {
        let p = BackupPlan {
            files_from: vec!["/list.txt".into()],
            retention: retention(),
            tags: vec!["t".into()],
            ..plan()
        };
        assert!(validate_plan_sources(&p).is_ok());
    }

    #[test]
    fn accepts_paths_with_retention_and_no_tags() {
        let p = BackupPlan { retention: retention(), ..plan() };
        assert!(validate_plan_sources(&p).is_ok());
    }
}
