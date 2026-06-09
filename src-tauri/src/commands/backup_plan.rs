use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPlan {
    pub id: String,
    pub name: String,
    pub repo_id: String,
    pub paths: Vec<String>,
    pub tags: Vec<String>,
    pub excludes: Vec<String>,
}

#[tauri::command]
pub async fn list_backup_plans(app: AppHandle) -> Result<Vec<BackupPlan>, String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    let plans: Vec<BackupPlan> = store
        .get("backup_plans")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    Ok(plans)
}

#[tauri::command]
pub async fn save_backup_plan(app: AppHandle, plan: BackupPlan) -> Result<(), String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    let mut plans: Vec<BackupPlan> = store
        .get("backup_plans")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    if let Some(idx) = plans.iter().position(|p| p.id == plan.id) {
        plans[idx] = plan;
    } else {
        plans.push(plan);
    }
    store.set(
        "backup_plans",
        serde_json::to_value(&plans).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn remove_backup_plan(app: AppHandle, plan_id: String) -> Result<(), String> {
    let store = app.store("settings.json").map_err(|e| e.to_string())?;
    let mut plans: Vec<BackupPlan> = store
        .get("backup_plans")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    plans.retain(|p| p.id != plan_id);
    store.set(
        "backup_plans",
        serde_json::to_value(&plans).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}
