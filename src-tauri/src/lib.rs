mod commands;
mod scheduler;

use commands::{auth, backup_plan, browse, cache, repo, schedule, snapshot};
use rusqlite::Connection;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};

struct MenuState {
    app_submenu: tauri::menu::Submenu<tauri::Wry>,
    file_submenu: tauri::menu::Submenu<tauri::Wry>,
    settings: tauri::menu::MenuItem<tauri::Wry>,
    new_repository: tauri::menu::MenuItem<tauri::Wry>,
    new_backup_plan: tauri::menu::MenuItem<tauri::Wry>,
    reset_app: tauri::menu::MenuItem<tauri::Wry>,
}

#[tauri::command]
fn set_menu_auth_state(unlocked: bool, menu_state: tauri::State<MenuState>) -> Result<(), String> {
    // Remove all managed items first (ignore errors if already absent)
    let _ = menu_state.app_submenu.remove(&menu_state.settings);
    let _ = menu_state.file_submenu.remove(&menu_state.new_repository);
    let _ = menu_state.file_submenu.remove(&menu_state.new_backup_plan);
    let _ = menu_state.file_submenu.remove(&menu_state.reset_app);

    if unlocked {
        menu_state.app_submenu.prepend(&menu_state.settings).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.new_repository).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.new_backup_plan).map_err(|e| e.to_string())?;
    } else {
        menu_state.file_submenu.append(&menu_state.reset_app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Resty Desktop")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?;
            let app_submenu = SubmenuBuilder::new(app, "Resty Desktop")
                .item(&quit)
                .build()?;
            let new_repo = MenuItemBuilder::with_id("new_repository", "New Repository").build(app)?;
            let new_backup_plan = MenuItemBuilder::with_id("new_backup_plan", "New Backup Plan").build(app)?;
            let reset_app_item = MenuItemBuilder::with_id("reset_app", "Reset Application").build(app)?;
            let file_submenu = SubmenuBuilder::new(app, "File")
                .item(&reset_app_item)
                .build()?;
            let edit_submenu = SubmenuBuilder::new(app, "Edit")
                .item(&PredefinedMenuItem::undo(app, None)?)
                .item(&PredefinedMenuItem::redo(app, None)?)
                .separator()
                .item(&PredefinedMenuItem::cut(app, None)?)
                .item(&PredefinedMenuItem::copy(app, None)?)
                .item(&PredefinedMenuItem::paste(app, None)?)
                .item(&PredefinedMenuItem::select_all(app, None)?)
                .build()?;
            let menu = MenuBuilder::new(app).items(&[&app_submenu, &file_submenu, &edit_submenu]).build()?;
            app.set_menu(menu)?;
            app.manage(MenuState {
                app_submenu,
                file_submenu,
                settings,
                new_repository: new_repo,
                new_backup_plan,
                reset_app: reset_app_item,
            });
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let conn = Connection::open(data_dir.join("app_data.db"))?;
            cache::AppDb::init_schema(&conn)?;
            let app_db = cache::AppDb::new(conn);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let _ = app_db.recalculate_overdue_schedules(now);
            app.manage(app_db);
            app.manage(cache::MasterKey::new());
            app.manage(cache::CopyHandle::new());
            app.manage(cache::MirrorHandle::new());
            app.manage(cache::BackupHandle::new());
            app.manage(cache::PruneHandle::new());
            scheduler::spawn(app.handle().clone());
            Ok(())
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "new_repository" => { app.emit("menu:new-repository", ()).ok(); }
                "new_backup_plan" => { app.emit("menu:new-backup-plan", ()).ok(); }
                "settings" => { app.emit("menu:settings", ()).ok(); }
                "reset_app" => { app.emit("menu:reset-app", ()).ok(); }
                "quit" => { app.exit(0); }
                _ => {}
            }
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
            repo::update_repo_path,
            repo::get_repo_password,
            repo::update_repo_password,
            repo::test_repo_connection,
            repo::get_repo_stats,
            repo::refresh_repo_stats,
            repo::get_restic_path,
            repo::set_restic_path,
            repo::get_restic_version,
            repo::get_compression,
            repo::set_compression,
            repo::check_repo,
            repo::prune_all_repos,
            repo::cancel_prune,
            // snapshots
            snapshot::list_snapshots,
            snapshot::refresh_snapshots,
            snapshot::delete_snapshot,
            snapshot::tag_snapshot,
            snapshot::run_backup,
            snapshot::forget_by_plan,
            snapshot::unlock_repo,
            snapshot::copy_snapshot,
            snapshot::cancel_copy,
            snapshot::mirror_repo,
            snapshot::cancel_mirror,
            snapshot::cancel_backup,
            // browse
            browse::list_files,
            browse::restore_path,
            browse::restore_snapshot,
            // backup plans
            backup_plan::list_backup_plans,
            backup_plan::save_backup_plan,
            backup_plan::remove_backup_plan,
            // schedules
            schedule::list_schedules,
            schedule::save_schedule,
            schedule::remove_schedule,
            schedule::toggle_schedule,
            schedule::run_schedule_now,
            schedule::describe_cron_expr,
            // cache
            cache::clear_browse_cache,
            cache::list_backup_history,
            // menu
            set_menu_auth_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
