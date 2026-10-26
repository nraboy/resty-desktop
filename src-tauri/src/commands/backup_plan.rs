use serde::{Deserialize, Serialize};
use tauri::State;

use super::cache::{AppDb, BackupPlan, RetentionPolicy};

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

/// The Settings "Default Retention Policy": a template the plan editor copies into a *new*
/// plan's retention fields. It is never consulted when a backup or `forget` runs — retention
/// deletes snapshots, so editing it must not change what existing plans prune.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DefaultRetention {
    pub enabled: bool,
    pub policy: RetentionPolicy,
}

const KEY_DEFAULT_RETENTION_ENABLED: &str = "default_retention_enabled";
const KEY_DEFAULT_RETENTION: &str = "default_retention";

/// Reads both keys under one `get_settings` call. Fails *closed*: a DB error, a missing key or
/// unparseable JSON all yield `Default` (off, no rules) so nothing is prefilled — the opposite of
/// `notify::load`, because a wrongly-prefilled retention policy can delete snapshots while a
/// wrongly-silent notification cannot.
pub fn load_default_retention(db: &AppDb) -> DefaultRetention {
    let rows = db
        .get_settings(&[KEY_DEFAULT_RETENTION_ENABLED, KEY_DEFAULT_RETENTION])
        .unwrap_or_default();
    DefaultRetention {
        enabled: rows.get(KEY_DEFAULT_RETENTION_ENABLED).map(|v| v == "true").unwrap_or(false),
        policy: rows
            .get(KEY_DEFAULT_RETENTION)
            .and_then(|v| serde_json::from_str(v).ok())
            .unwrap_or_default(),
    }
}

/// Writes both keys in one transaction. `Some(0)` is folded to unset, matching
/// `has_effective_retention`. Saving `enabled: false` keeps the policy so switching back on
/// restores it.
pub fn save_default_retention(db: &AppDb, value: DefaultRetention) -> Result<(), String> {
    let nz = |n: Option<u32>| n.filter(|n| *n > 0);
    let p = value.policy;
    let policy = RetentionPolicy {
        keep_last: nz(p.keep_last),
        keep_daily: nz(p.keep_daily),
        keep_weekly: nz(p.keep_weekly),
        keep_monthly: nz(p.keep_monthly),
        keep_yearly: nz(p.keep_yearly),
    };
    let json = serde_json::to_string(&policy).map_err(|e| e.to_string())?;
    db.set_settings(&[
        (KEY_DEFAULT_RETENTION_ENABLED, if value.enabled { "true" } else { "false" }),
        (KEY_DEFAULT_RETENTION, &json),
    ])
}

#[tauri::command]
pub fn get_default_retention(db: State<'_, AppDb>) -> Result<DefaultRetention, String> {
    Ok(load_default_retention(&db))
}

#[tauri::command]
pub fn set_default_retention(db: State<'_, AppDb>, value: DefaultRetention) -> Result<(), String> {
    save_default_retention(&db, value)
}

#[cfg(test)]
mod tests {
    use super::{load_default_retention, save_default_retention, validate_plan_sources, DefaultRetention};
    use crate::commands::cache::{AppDb, BackupPlan, RetentionPolicy};

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

    fn test_db() -> AppDb {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        AppDb::init_schema(&conn).unwrap();
        AppDb::new(conn, std::path::PathBuf::new())
    }

    #[test]
    fn default_retention_on_empty_db_is_off_and_empty() {
        let d = load_default_retention(&test_db());
        assert!(!d.enabled);
        assert!(d.policy.keep_last.is_none() && d.policy.keep_yearly.is_none());
    }

    #[test]
    fn default_retention_round_trips() {
        let db = test_db();
        let policy = RetentionPolicy { keep_daily: Some(7), keep_weekly: Some(4), ..Default::default() };
        save_default_retention(&db, DefaultRetention { enabled: true, policy }).unwrap();
        let d = load_default_retention(&db);
        assert!(d.enabled);
        assert_eq!(d.policy.keep_daily, Some(7));
        assert_eq!(d.policy.keep_weekly, Some(4));
        assert_eq!(d.policy.keep_last, None);
    }

    #[test]
    fn default_retention_folds_zero_to_unset() {
        let db = test_db();
        let policy = RetentionPolicy { keep_last: Some(0), keep_daily: Some(3), ..Default::default() };
        save_default_retention(&db, DefaultRetention { enabled: true, policy }).unwrap();
        let d = load_default_retention(&db);
        assert_eq!(d.policy.keep_last, None);
        assert_eq!(d.policy.keep_daily, Some(3));
    }

    #[test]
    fn disabling_default_retention_keeps_the_policy() {
        let db = test_db();
        let policy = RetentionPolicy { keep_monthly: Some(12), ..Default::default() };
        save_default_retention(&db, DefaultRetention { enabled: true, policy: policy.clone() }).unwrap();
        save_default_retention(&db, DefaultRetention { enabled: false, policy }).unwrap();
        let d = load_default_retention(&db);
        assert!(!d.enabled);
        assert_eq!(d.policy.keep_monthly, Some(12));
    }

    #[test]
    fn corrupt_default_retention_json_fails_closed() {
        let db = test_db();
        db.set_settings(&[("default_retention_enabled", "true"), ("default_retention", "{not json")])
            .unwrap();
        let d = load_default_retention(&db);
        assert!(d.policy.keep_last.is_none() && d.policy.keep_daily.is_none());
    }
}
