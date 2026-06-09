mod commands;

use commands::{backup_plan, browse, cache, repo, snapshot};
use rusqlite::Connection;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = Connection::open(data_dir.join("browse_cache.db"))?;
            cache::SnapshotCache::init_schema(&conn)?;
            app.manage(cache::SnapshotCache::new(conn));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            repo::list_repos,
            repo::add_repo,
            repo::remove_repo,
            repo::init_repo,
            repo::get_repo_stats,
            repo::refresh_repo_stats,
            repo::rename_repo,
            repo::check_repo,
            repo::get_restic_path,
            repo::set_restic_path,
            snapshot::list_snapshots,
            snapshot::refresh_snapshots,
            snapshot::delete_snapshot,
            snapshot::tag_snapshot,
            snapshot::run_backup,
            snapshot::forget_by_plan,
            browse::list_files,
            browse::restore_path,
            backup_plan::list_backup_plans,
            backup_plan::save_backup_plan,
            backup_plan::remove_backup_plan,
            cache::clear_browse_cache,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
