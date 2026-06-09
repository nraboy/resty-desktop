pub mod backup_plan;
pub mod browse;
pub mod repo;
pub mod snapshot;

use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

pub(super) fn get_restic_path(app: &AppHandle) -> String {
    app.store("settings.json")
        .ok()
        .and_then(|s| s.get("restic_path"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_else(|| "restic".to_string())
}
