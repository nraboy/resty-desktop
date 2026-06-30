mod cache_warmer;
mod commands;
mod scheduler;

use commands::{auth, backup_plan, browse, cache, repo, schedule, snapshot, transfer};
use rusqlite::Connection;
use std::sync::Mutex;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};
use tauri::tray::TrayIconBuilder;

struct MenuState {
    app_submenu: tauri::menu::Submenu<tauri::Wry>,
    file_submenu: tauri::menu::Submenu<tauri::Wry>,
    settings: tauri::menu::MenuItem<tauri::Wry>,
    new_repository: tauri::menu::MenuItem<tauri::Wry>,
    new_backup_plan: tauri::menu::MenuItem<tauri::Wry>,
    reset_app: tauri::menu::MenuItem<tauri::Wry>,
    file_separator: tauri::menu::PredefinedMenuItem<tauri::Wry>,
    import_item: tauri::menu::MenuItem<tauri::Wry>,
    export_item: tauri::menu::MenuItem<tauri::Wry>,
}

// Keeps the TrayIcon alive; None until the app has been unlocked.
struct TrayState(Mutex<Option<tauri::tray::TrayIcon<tauri::Wry>>>);

static TRAY_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn show_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn set_menu_auth_state(unlocked: bool, menu_state: tauri::State<MenuState>) -> Result<(), String> {
    let _ = menu_state.app_submenu.remove(&menu_state.settings);
    let _ = menu_state.file_submenu.remove(&menu_state.new_repository);
    let _ = menu_state.file_submenu.remove(&menu_state.new_backup_plan);
    let _ = menu_state.file_submenu.remove(&menu_state.reset_app);
    let _ = menu_state.file_submenu.remove(&menu_state.file_separator);
    let _ = menu_state.file_submenu.remove(&menu_state.import_item);
    let _ = menu_state.file_submenu.remove(&menu_state.export_item);

    if unlocked {
        menu_state.app_submenu.prepend(&menu_state.settings).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.new_repository).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.new_backup_plan).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.file_separator).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.import_item).map_err(|e| e.to_string())?;
        menu_state.file_submenu.append(&menu_state.export_item).map_err(|e| e.to_string())?;
    } else {
        menu_state.file_submenu.append(&menu_state.reset_app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Removes the tray icon (called when the user disables the tray toggle).
/// On Windows, set_visible(false) maps to NIM_DELETE and removes the icon synchronously.
/// We then forget the value to skip Drop, which would otherwise issue a second NIM_DELETE
/// and log "Error removing system tray icon". On macOS, Drop handles removal cleanly.
#[tauri::command]
fn deactivate_tray(tray_state: tauri::State<TrayState>) -> Result<(), String> {
    let mut guard = tray_state.0.lock().map_err(|e| e.to_string())?;
    if let Some(tray) = guard.take() {
        let _ = tray.set_visible(false);
        // On Windows, set_visible(false) maps to NIM_DELETE (full OS removal), so Drop
        // would issue a second NIM_DELETE and log an error — skip it with mem::forget.
        // On macOS/Linux, set_visible(false) only hides the icon; Drop then cleans up
        // the remaining resources cleanly with no double-removal.
        #[cfg(target_os = "windows")]
        std::mem::forget(tray);
    }
    Ok(())
}

/// Called from the frontend after successful unlock/setup, and when the tray toggle is
/// turned on. Always recreates fresh — set_visible(true) is unreliable on macOS.
/// The caller is responsible for checking tray_enabled before invoking.
#[tauri::command]
fn activate_tray(
    app: tauri::AppHandle,
    tray_state: tauri::State<TrayState>,
) -> Result<(), String> {
    let mut guard = tray_state.0.lock().map_err(|e| e.to_string())?;
    // Drop any existing icon before recreating.
    *guard = None;
    // Use a unique generation suffix so menu item IDs don't collide with the previous instance.
    let gen = TRAY_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let open_id = format!("tray_open_{gen}");
    let settings_id = format!("tray_settings_{gen}");
    let quit_id = format!("tray_quit_{gen}");
    let tray_open = MenuItemBuilder::with_id(&open_id, "Open").build(&app).map_err(|e| e.to_string())?;
    let tray_settings = MenuItemBuilder::with_id(&settings_id, "Settings").build(&app).map_err(|e| e.to_string())?;
    let tray_quit = MenuItemBuilder::with_id(&quit_id, "Quit Resty Desktop").build(&app).map_err(|e| e.to_string())?;
    let tray_menu = MenuBuilder::new(&app)
        .item(&tray_open)
        .item(&tray_settings)
        .separator()
        .item(&tray_quit)
        .build()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let png_bytes = include_bytes!("../icons/tray-icon.png");
    #[cfg(not(target_os = "macos"))]
    let png_bytes = include_bytes!("../icons/32x32.png");
    let decoded = image::load_from_memory(png_bytes)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = decoded.dimensions();
    let icon = tauri::image::Image::new_owned(decoded.into_raw(), w, h);
    let tray = TrayIconBuilder::new()
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .show_menu_on_left_click(true)
        .tooltip("Resty Desktop")
        .menu(&tray_menu)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            if id == open_id || id == settings_id {
                show_window(app);
                if id == settings_id {
                    app.emit("menu:settings", ()).ok();
                }
            } else if id == quit_id {
                app.exit(0);
            }
        })
        .build(&app)
        .map_err(|e| e.to_string())?;
    *guard = Some(tray);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }))
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
            let file_separator = PredefinedMenuItem::separator(app)?;
            let import_item = MenuItemBuilder::with_id("import", "Import…").build(app)?;
            let export_item = MenuItemBuilder::with_id("export", "Export…").build(app)?;
            let file_submenu = SubmenuBuilder::new(app, "File")
                .item(&reset_app_item)
                .build()?;
            let source_github = MenuItemBuilder::with_id("source_github", "Source on GitHub").build(app)?;
            let help_submenu = SubmenuBuilder::new(app, "Help")
                .item(&source_github)
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
            let menu = MenuBuilder::new(app).items(&[&app_submenu, &file_submenu, &edit_submenu, &help_submenu]).build()?;
            // Native menu bar on Linux inherits the GTK theme and can be unreadable when
            // the GTK dark-theme hint conflicts with text color. All navigation is in the
            // sidebar, so skip the menu bar on Linux entirely.
            #[cfg(not(target_os = "linux"))]
            app.set_menu(menu)?;
            #[cfg(target_os = "linux")]
            drop(menu);
            app.manage(MenuState {
                app_submenu,
                file_submenu,
                settings,
                new_repository: new_repo,
                new_backup_plan,
                reset_app: reset_app_item,
                file_separator,
                import_item,
                export_item,
            });
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("app_data.db");
            let conn = Connection::open(&db_path)?;
            cache::AppDb::init_schema(&conn)?;
            let app_db = cache::AppDb::new(conn, db_path);
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

            // Tray is created lazily after unlock via activate_tray command.
            app.manage(TrayState(Mutex::new(None)));

            // Intercept window close: hide to tray only after tray has been activated
            // (i.e. the user has unlocked the app). Before unlock, close quits the app.
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                let win = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        let tray = app_handle.state::<TrayState>();
                        let tray_active = tray.0.lock().map(|g| g.is_some()).unwrap_or(false);
                        if tray_active {
                            let db = app_handle.state::<cache::AppDb>();
                            let tray_on = db
                                .get_setting("tray_enabled", "false")
                                .unwrap_or_else(|_| "false".to_string())
                                == "true";
                            if tray_on {
                                api.prevent_close();
                                let _ = win.hide();
                                #[cfg(target_os = "macos")]
                                let _ = app_handle.set_activation_policy(tauri::ActivationPolicy::Accessory);
                            }
                        }
                    }
                });
            }

            scheduler::spawn(app.handle().clone());
            cache_warmer::spawn(app.handle().clone());
            Ok(())
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "new_repository" => { app.emit("menu:new-repository", ()).ok(); }
                "new_backup_plan" => { app.emit("menu:new-backup-plan", ()).ok(); }
                "settings" => { app.emit("menu:settings", ()).ok(); }
                "reset_app" => { app.emit("menu:reset-app", ()).ok(); }
                "import" => { app.emit("menu:import", ()).ok(); }
                "export" => { app.emit("menu:export", ()).ok(); }
                "source_github" => { app.emit("menu:source-github", ()).ok(); }
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
            repo::get_restore_path,
            repo::set_restore_path,
            repo::get_tray_enabled,
            repo::set_tray_enabled,
            repo::get_tray_warning,
            repo::get_remote_auto_refresh,
            repo::set_remote_auto_refresh,
            repo::get_auto_indexing,
            repo::set_auto_indexing,
            repo::check_repo,
            repo::prune_all_repos,
            repo::prune_repo,
            repo::cancel_prune,
            repo::check_full_disk_access,
            repo::open_full_disk_access_settings,
            // snapshots
            snapshot::list_snapshots,
            snapshot::refresh_snapshots,
            snapshot::delete_snapshot,
            snapshot::tag_snapshot,
            snapshot::get_snapshot_stats,
            snapshot::run_backup,
            snapshot::forget_by_plan,
            snapshot::unlock_repo,
            snapshot::copy_snapshot,
            snapshot::cancel_copy,
            snapshot::mirror_repo,
            snapshot::cancel_mirror,
            snapshot::cancel_backup,
            snapshot::diff_snapshots,
            // browse
            browse::list_files,
            browse::restore_path,
            browse::restore_snapshot,
            browse::index_snapshot,
            browse::search_snapshot_files,
            browse::get_snapshot_index_status,
            browse::clear_snapshot_index,
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
            cache::clean_cache,
            cache::get_db_size,
            cache::list_backup_history,
            // import / export
            transfer::export_data,
            transfer::preview_import,
            transfer::import_data,
            transfer::preview_backrest_import,
            transfer::import_backrest_config,
            // menu / tray
            set_menu_auth_state,
            activate_tray,
            deactivate_tray,
        ])
        .build(tauri::generate_context!())
        .expect("error building tauri application")
        .run(|app_handle, event| {
            // macOS dock click while window is hidden — restore window and dock presence
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows, .. } = event {
                if !has_visible_windows {
                    show_window(app_handle);
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app_handle, event);
        });
}
