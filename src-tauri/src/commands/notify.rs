//! Global notification preferences. Four toggles, stored as `"true"`/`"false"` strings in the
//! existing `app_settings` key/value table (no schema change needed — see CLAUDE.md's
//! Persistence & Caching section). `execute_backup` (snapshot.rs) is the only caller of `notify`
//! today; every other operation (mirror, prune, check, restore, index) still shows no
//! notification at all.

use serde::{Deserialize, Serialize};
use tauri::State;

use super::cache::AppDb;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSettings {
    pub started: bool,
    pub success_changed: bool,
    pub success_unchanged: bool,
    pub failures: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            started: false,
            success_changed: true,
            success_unchanged: false,
            failures: true,
        }
    }
}

impl NotificationSettings {
    pub fn allows(&self, cat: NotifyCategory) -> bool {
        match cat {
            NotifyCategory::Started => self.started,
            NotifyCategory::SuccessChanged => self.success_changed,
            NotifyCategory::SuccessUnchanged => self.success_unchanged,
            NotifyCategory::Failures => self.failures,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyCategory {
    Started,
    SuccessChanged,
    SuccessUnchanged,
    Failures,
}

const KEY_STARTED: &str = "notify_started";
const KEY_SUCCESS_CHANGED: &str = "notify_success_changed";
const KEY_SUCCESS_UNCHANGED: &str = "notify_success_unchanged";
const KEY_FAILURES: &str = "notify_failures";

fn bool_str(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

/// Reads all four notification settings under a single `AppDb::get_settings` call (one mutex
/// acquisition/query for all four, not four separate `get_setting` round-trips). Fails *open* on
/// any DB error (i.e. returns `Default`, which shows started=off/success_changed=on/
/// success_unchanged=off/failures=on) rather than silently suppressing a failure
/// notification because the settings table couldn't be read.
pub fn load(db: &AppDb) -> NotificationSettings {
    let defaults = NotificationSettings::default();
    let rows = db
        .get_settings(&[KEY_STARTED, KEY_SUCCESS_CHANGED, KEY_SUCCESS_UNCHANGED, KEY_FAILURES])
        .unwrap_or_default();
    let get = |key: &str, default: bool| -> bool {
        rows.get(key).map(|v| v == "true").unwrap_or(default)
    };
    NotificationSettings {
        started: get(KEY_STARTED, defaults.started),
        success_changed: get(KEY_SUCCESS_CHANGED, defaults.success_changed),
        success_unchanged: get(KEY_SUCCESS_UNCHANGED, defaults.success_unchanged),
        failures: get(KEY_FAILURES, defaults.failures),
    }
}

/// Writes all four keys atomically (`AppDb::set_settings`, one transaction) so a full-object
/// save from one `updateNotifications` call can never interleave its four writes with another
/// concurrent save's — whichever call's transaction commits last cleanly wins in full, rather
/// than leaving a mix of fields from two different saves.
pub fn save(db: &AppDb, value: NotificationSettings) -> Result<(), String> {
    db.set_settings(&[
        (KEY_STARTED, bool_str(value.started)),
        (KEY_SUCCESS_CHANGED, bool_str(value.success_changed)),
        (KEY_SUCCESS_UNCHANGED, bool_str(value.success_unchanged)),
        (KEY_FAILURES, bool_str(value.failures)),
    ])
}

/// Pure classification of a successful (exit 0) backup into the right notify category.
/// "No changes" is `files_new == 0 && files_changed == 0`, deliberately not consulting
/// `data_added`/dir counts, which restic can report nonzero on genuine no-op runs.
pub fn classify_success(files_new: u64, files_changed: u64) -> NotifyCategory {
    if files_new + files_changed > 0 {
        NotifyCategory::SuccessChanged
    } else {
        NotifyCategory::SuccessUnchanged
    }
}

/// Single gated entry point for every backup notification. Needs an `AppHandle` so it can't be
/// unit-tested, and every branch here is fire-and-forget, matching the pre-existing
/// `let _ = ... .show()` sites it replaces.
pub fn notify(app: &tauri::AppHandle, db: &AppDb, cat: NotifyCategory, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if !load(db).allows(cat) {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}

#[tauri::command]
pub fn get_notification_settings(db: State<'_, AppDb>) -> Result<NotificationSettings, String> {
    Ok(load(&db))
}

#[tauri::command]
pub fn set_notification_settings(
    db: State<'_, AppDb>,
    value: NotificationSettings,
) -> Result<(), String> {
    save(&db, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> AppDb {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        AppDb::init_schema(&conn).unwrap();
        AppDb::new(conn, std::path::PathBuf::new())
    }

    #[test]
    fn classify_success_no_changes_is_success_unchanged() {
        assert_eq!(classify_success(0, 0), NotifyCategory::SuccessUnchanged);
    }

    #[test]
    fn classify_success_with_changes_is_success_changed() {
        assert_eq!(classify_success(3, 0), NotifyCategory::SuccessChanged);
        assert_eq!(classify_success(0, 2), NotifyCategory::SuccessChanged);
    }

    #[test]
    fn allows_matches_each_category_to_its_own_flag() {
        let settings = NotificationSettings {
            started: true,
            success_changed: false,
            success_unchanged: true,
            failures: true,
        };
        assert!(settings.allows(NotifyCategory::Started));
        assert!(!settings.allows(NotifyCategory::SuccessChanged));
        assert!(settings.allows(NotifyCategory::SuccessUnchanged));
        assert!(settings.allows(NotifyCategory::Failures));
    }

    #[test]
    fn load_on_empty_db_returns_defaults() {
        let db = test_db();
        assert_eq!(load(&db), NotificationSettings::default());
    }

    #[test]
    fn save_then_load_round_trips() {
        let db = test_db();
        let value = NotificationSettings {
            started: true,
            success_changed: false,
            success_unchanged: true,
            failures: true,
        };
        save(&db, value).unwrap();
        assert_eq!(load(&db), value);
    }

    #[test]
    fn load_after_partial_save_falls_back_per_key() {
        // Only one key written directly (bypassing `save`, which always writes all four) — the
        // batched AppDb::get_settings query must still return per-key defaults for the rest,
        // not fail the whole load because some keys are absent from the result map.
        let db = test_db();
        db.set_setting(KEY_FAILURES, "false").unwrap();
        let loaded = load(&db);
        assert!(!loaded.failures);
        assert_eq!(loaded, NotificationSettings { failures: false, ..NotificationSettings::default() });
    }
}
