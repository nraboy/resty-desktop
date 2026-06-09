mod commands;

use commands::{auth, backup_plan, browse, cache, repo, snapshot};
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
            let conn = Connection::open(data_dir.join("app_data.db"))?;
            cache::AppDb::init_schema(&conn)?;
            app.manage(cache::AppDb::new(conn));
            app.manage(cache::MasterKey::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // auth
            auth::is_app_setup,
            auth::setup_master_password,
            auth::unlock_app,
            auth::lock_app,
            auth::change_master_password,
            auth::reset_app,
            // repos
            repo::list_repos,
            repo::add_repo,
            repo::remove_repo,
            repo::init_repo,
            repo::rename_repo,
            repo::test_repo_connection,
            repo::get_repo_stats,
            repo::refresh_repo_stats,
            repo::get_restic_path,
            repo::set_restic_path,
            // snapshots
            snapshot::list_snapshots,
            snapshot::refresh_snapshots,
            snapshot::delete_snapshot,
            snapshot::tag_snapshot,
            snapshot::run_backup,
            snapshot::forget_by_plan,
            // browse
            browse::list_files,
            browse::restore_path,
            // backup plans
            backup_plan::list_backup_plans,
            backup_plan::save_backup_plan,
            backup_plan::remove_backup_plan,
            // cache
            cache::clear_browse_cache,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
