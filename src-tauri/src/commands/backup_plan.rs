use tauri::State;

use super::cache::{AppDb, BackupPlan};

#[tauri::command]
pub fn list_backup_plans(db: State<'_, AppDb>) -> Result<Vec<BackupPlan>, String> {
    db.list_backup_plans()
}

#[tauri::command]
pub fn save_backup_plan(db: State<'_, AppDb>, plan: BackupPlan) -> Result<(), String> {
    db.save_backup_plan(&plan)
}

#[tauri::command]
pub fn remove_backup_plan(db: State<'_, AppDb>, plan_id: String) -> Result<(), String> {
    db.remove_backup_plan(&plan_id)
}
