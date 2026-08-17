mod cache_warmer;
mod commands;
mod gpu_compat;
mod scheduler;
mod tasks;

use commands::{
    auth, backup_plan, browse, cache, notify, repo, repo_locks, schedule, snapshot, transfer, webhook,
};
use rusqlite::Connection;
use std::sync::Mutex;
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};
use tauri::tray::TrayIconBuilder;
use tauri_plugin_autostart::ManagerExt;

struct MenuState {
    app_submenu: tauri::menu::Submenu<tauri::Wry>,
    file_submenu: tauri::menu::Submenu<tauri::Wry>,
    settings: tauri::menu::MenuItem<tauri::Wry>,
    lock_app: tauri::menu::MenuItem<tauri::Wry>,
    new_repository: tauri::menu::MenuItem<tauri::Wry>,
    new_backup_plan: tauri::menu::MenuItem<tauri::Wry>,
    reset_app: tauri::menu::MenuItem<tauri::Wry>,
    file_separator: tauri::menu::PredefinedMenuItem<tauri::Wry>,
    import_item: tauri::menu::MenuItem<tauri::Wry>,
    export_item: tauri::menu::MenuItem<tauri::Wry>,
}

/// Holds the live TrayIcon plus which variant is installed, under one lock so the two can
/// never disagree. `unlocked` is `None` exactly when `icon` is `None`. `gen` is the generation
/// suffix baked into the live icon's `on_menu_event` closure at build time — every later
/// `set_menu` call must reuse it so the closure's captured ids keep matching the swapped-in menu.
#[derive(Default)]
struct Tray {
    icon: Option<tauri::tray::TrayIcon<tauri::Wry>>,
    unlocked: Option<bool>,
    gen: u32,
}
struct TrayState(Mutex<Tray>);

static TRAY_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn show_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// A login launch starts hidden only when all three are on: the OS-autostart marker, the tray
/// setting (there is otherwise no icon to bring the window back), and auto-unlock (otherwise
/// the app would sit locked and invisible doing nothing — the exact failure mode the old
/// "never launch hidden" decision existed to prevent). See docs/decisions.md. Pulled out as its
/// own function, rather than left inline in `setup()`, specifically so it's unit-testable —
/// don't drop any of the three conditions without updating `should_start_hidden_requires_all_three`.
fn should_start_hidden(from_autostart: bool, tray_on: bool, auto_unlock_on: bool) -> bool {
    from_autostart && tray_on && auto_unlock_on
}

/// Frontend-callable wrapper over `show_window`. Idempotent — showing an already-visible
/// window is a no-op — so App.tsx can call it unconditionally on every non-unlocked auth
/// state without tracking whether this particular launch started hidden.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    show_window(&app);
}

#[tauri::command]
fn set_menu_auth_state(unlocked: bool, menu_state: tauri::State<MenuState>) -> Result<(), String> {
    let _ = menu_state.app_submenu.remove(&menu_state.settings);
    let _ = menu_state.app_submenu.remove(&menu_state.lock_app);
    let _ = menu_state.file_submenu.remove(&menu_state.new_repository);
    let _ = menu_state.file_submenu.remove(&menu_state.new_backup_plan);
    let _ = menu_state.file_submenu.remove(&menu_state.reset_app);
    let _ = menu_state.file_submenu.remove(&menu_state.file_separator);
    let _ = menu_state.file_submenu.remove(&menu_state.import_item);
    let _ = menu_state.file_submenu.remove(&menu_state.export_item);

    if unlocked {
        // prepend inserts at index 0 each call, so prepending in this order — lock_app first,
        // then settings — yields the intended top-to-bottom layout: Settings, Lock Now, … Quit.
        menu_state.app_submenu.prepend(&menu_state.lock_app).map_err(|e| e.to_string())?;
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

/// Removes the tray icon (called when the user disables the tray toggle, or on app reset).
///
/// `tauri::tray::TrayIcon` wraps a reference-counted `tray_icon::TrayIcon` (`Rc<RefCell<..>>`
/// under the hood); building it via `TrayIconBuilder::build` also stores a second clone in
/// Tauri's own resource table (see `TrayIcon::register`), so simply dropping our stored handle
/// does *not* remove the OS icon — the resource-table clone keeps it alive. `remove_tray_by_id`
/// takes that second clone out of the table; dropping both the returned clone and our own then
/// actually frees the platform icon. Must run on the main thread (macOS removal touches AppKit).
#[tauri::command]
fn deactivate_tray(app: tauri::AppHandle, tray_state: tauri::State<TrayState>) -> Result<(), String> {
    let mut guard = tray_state.0.lock().map_err(|e| e.to_string())?;
    if let Some(tray) = guard.icon.take() {
        let id = tray.id().clone();
        let app_for_thread = app.clone();
        let _ = app.run_on_main_thread(move || {
            let _table_clone = app_for_thread.remove_tray_by_id(&id);
            drop(tray);
        });
    }
    guard.unlocked = None;
    Ok(())
}

/// Builds the tray's context menu for the given generation and auth state. Split out of
/// `build_tray` so `activate_tray` can swap variants in place with `TrayIcon::set_menu`
/// instead of rebuilding the icon (see `activate_tray`'s doc comment for why rebuilding is
/// wrong here). `gen` must be the live icon's generation — its `on_menu_event` closure
/// recomputes the same ids from `gen` at click time, so menu and closure only agree when
/// they share a generation.
fn build_tray_menu(app: &tauri::AppHandle, gen: u32, unlocked: bool) -> Result<tauri::menu::Menu<tauri::Wry>, String> {
    let open_id = format!("tray_open_{gen}");
    let settings_id = format!("tray_settings_{gen}");
    let lock_id = format!("tray_lock_{gen}");
    let quit_id = format!("tray_quit_{gen}");

    let tray_open = MenuItemBuilder::with_id(&open_id, "Open").build(app).map_err(|e| e.to_string())?;
    let tray_quit = MenuItemBuilder::with_id(&quit_id, "Quit Resty Desktop").build(app).map_err(|e| e.to_string())?;

    // "Settings" fires the `menu:settings` event, which only App.tsx's unlocked subtree
    // listens for — on the lock screen it would open the window onto a dead click, so the
    // item is not built at all (the on_menu_event closure simply never matches its id).
    let tray_settings = if unlocked {
        Some(MenuItemBuilder::with_id(&settings_id, "Settings").build(app).map_err(|e| e.to_string())?)
    } else {
        None
    };
    // "Lock Now" — unlocked only, same reasoning as Settings above. This is the only way to
    // lock a session that's sitting unlocked-and-hidden in the tray: on macOS the native menu
    // bar is unreachable while the window is hidden under ActivationPolicy::Accessory, and the
    // window itself may not be on screen at all. Fires the same `menu:lock-app` event the
    // native "Lock Now" menu item already emits (App.tsx's unlocked-only listener), so locking
    // from here deliberately does not show the window.
    let tray_lock = if unlocked {
        Some(MenuItemBuilder::with_id(&lock_id, "Lock Now").build(app).map_err(|e| e.to_string())?)
    } else {
        None
    };
    // A disabled header row, not just the tooltip: Linux StatusNotifierItem hosts (GNOME's
    // AppIndicator extension in particular) routinely ignore tray tooltips, so the locked
    // state has to be visible in the menu itself to read the same on all three platforms.
    let status_id = format!("tray_status_{gen}");
    let tray_status = if unlocked {
        None
    } else {
        // enabled(false) renders a greyed, unclickable row on all three platforms; its id is
        // never matched in on_menu_event because it can't be clicked.
        Some(
            MenuItemBuilder::with_id(&status_id, "Locked")
                .enabled(false)
                .build(app)
                .map_err(|e| e.to_string())?,
        )
    };

    let mut b = MenuBuilder::new(app);
    if let Some(item) = &tray_status {
        b = b.item(item).separator();
    }
    b = b.item(&tray_open);
    if let Some(item) = &tray_settings {
        b = b.item(item);
    }
    if let Some(item) = &tray_lock {
        b = b.item(item);
    }
    b.separator().item(&tray_quit).build().map_err(|e| e.to_string())
}

/// Builds a fresh tray icon (with its own generation) for the given auth state. Called only
/// when no icon currently exists: `setup()` (always the locked variant — the app always starts
/// locked) and `activate_tray`'s no-existing-icon branch. Returns the icon alongside its
/// generation so the caller can store it in `Tray::gen` for later `set_menu` calls.
fn build_tray(app: &tauri::AppHandle, unlocked: bool) -> Result<(tauri::tray::TrayIcon<tauri::Wry>, u32), String> {
    // Unique generation suffix so menu item IDs don't collide with a previous instance.
    let gen = TRAY_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let tray_menu = build_tray_menu(app, gen, unlocked)?;

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
        .tooltip(if unlocked { "Resty Desktop" } else { "Resty Desktop — Locked" })
        .menu(&tray_menu)
        .on_menu_event(move |app, event| {
            let id = event.id().as_ref();
            if id == format!("tray_open_{gen}") || id == format!("tray_settings_{gen}") {
                show_window(app);
                if id == format!("tray_settings_{gen}") {
                    app.emit("menu:settings", ()).ok();
                }
            } else if id == format!("tray_lock_{gen}") {
                // Deliberately does not call show_window — locking from the tray must be
                // able to leave a hidden session hidden (see build_tray_menu's doc comment).
                app.emit("menu:lock-app", ()).ok();
            } else if id == format!("tray_quit_{gen}") {
                app.exit(0);
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;
    Ok((tray, gen))
}

/// Called from the frontend on every auth-state change (and when the tray toggle is turned
/// on). Returns early when the requested variant is already installed — App.tsx runs this on
/// every `locked`/`unlocked` transition, and this also absorbs React StrictMode's dev-only
/// double-invoke.
///
/// When a variant switch is actually needed, this updates the **existing** icon in place via
/// `set_menu`/`set_tooltip` rather than rebuilding it. Rebuilding (dropping the stored handle
/// and calling `build_tray` again) does *not* remove the old OS icon: `TrayIconBuilder::build`
/// stores a second clone of the icon in Tauri's own resource table (see `TrayIcon::register`),
/// so dropping only our handle leaves that clone alive — the old icon stays in the menu bar
/// and a new one appears next to it. In-place updates also avoid a real Windows
/// `NIM_DELETE`/`NIM_ADD` pair (a visible icon flicker and a lost overflow-area position). See
/// `deactivate_tray` for the correct way to actually remove an icon.
#[tauri::command]
fn activate_tray(
    app: tauri::AppHandle,
    tray_state: tauri::State<TrayState>,
    unlocked: bool,
) -> Result<(), String> {
    let mut guard = tray_state.0.lock().map_err(|e| e.to_string())?;
    if guard.icon.is_some() && guard.unlocked == Some(unlocked) {
        return Ok(());
    }
    if let Some(icon) = guard.icon.clone() {
        let menu = build_tray_menu(&app, guard.gen, unlocked)?;
        icon.set_menu(Some(menu)).map_err(|e| e.to_string())?;
        // The menu is what actually carries the auth-state signal (Settings/Lock Now vs. the
        // disabled "Locked" row) — record the variant as soon as it lands, so a subsequent
        // set_tooltip failure (tooltip is decoration only, and a guaranteed no-op on Linux)
        // can't leave `guard.unlocked` disagreeing with the menu that's actually installed.
        guard.unlocked = Some(unlocked);
        icon.set_tooltip(Some(if unlocked { "Resty Desktop" } else { "Resty Desktop — Locked" }))
            .map_err(|e| e.to_string())?;
    } else {
        let (icon, gen) = build_tray(&app, unlocked)?;
        guard.icon = Some(icon);
        guard.gen = gen;
        guard.unlocked = Some(unlocked);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    gpu_compat::apply();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second *autostart* launch (Windows Run key plus a Startup-folder shortcut, a
            // desktop session that both restores and autostarts) must not yank a deliberately
            // hidden login launch onto the screen. Only a hand launch means "show me the app".
            if args.iter().any(|a| a == "--from-autostart") {
                return;
            }
            show_window(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        // app_name must be set explicitly: it defaults to package_info().name
        // ("Resty Desktop", with a space), and auto-launch writes the Linux Exec=
        // line and the Windows Run value unquoted. Also becomes the autostart
        // filename and the macOS plist Label. MacosLauncher::LaunchAgent is the
        // crate default (no TCC Automation prompt, unlike AppleScript mode), so
        // it's deliberately left unset here.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("resty-desktop")
                // Written into the OS autostart entry so a login launch is distinguishable
                // from a hand launch (see the single-instance handler above and setup()'s
                // start_hidden gate below). Verified honored on all three platforms by the
                // pinned auto-launch 0.5.0: macOS LaunchAgent ProgramArguments, the Windows
                // Run value, and the Linux Exec= line.
                .args(["--from-autostart"])
                .build(),
        )
        .setup(|app| {
            let settings = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
            let lock_app_item = MenuItemBuilder::with_id("lock_app", "Lock Now").build(app)?;
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
            let file_submenu = SubmenuBuilder::new(app, "File").item(&reset_app_item).build()?;
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
            // the GTK dark-theme hint conflicts with text color. On Windows the in-window
            // menu bar stacks the app name against the title bar and sidebar logo (the bug
            // the old File-fold workaround existed to kill) and adds nothing the sidebar
            // doesn't cover. All navigation is in the sidebar, so only macOS — whose menu
            // lives in the system bar, away from the window — installs it.
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            app.set_menu(menu)?;
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            drop(menu);
            app.manage(MenuState {
                app_submenu,
                file_submenu,
                settings,
                lock_app: lock_app_item,
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

            // ── startup visibility ──────────────────────────────────────────────────
            let tray_on = app_db
                .get_setting("tray_enabled", "false")
                .unwrap_or_else(|_| "false".to_string())
                == "true";
            let auto_unlock_on = app_db
                .get_setting("auto_unlock", "false")
                .unwrap_or_else(|_| "false".to_string())
                == "true";
            // Only the OS autostart entry carries this arg (see the plugin builder above).
            let from_autostart = std::env::args().any(|a| a == "--from-autostart");

            let start_hidden = should_start_hidden(from_autostart, tray_on, auto_unlock_on);

            // One-shot re-registration: entries written by an older build carry no
            // `--from-autostart`, so a hidden start would never trigger for existing users.
            // enable() truncates and rewrites the plist / Run value / .desktop file with the
            // current args. Guarded on is_enabled() so this never *creates* an entry, and —
            // on Windows — never resurrects one the user switched off in Task Manager
            // (is_enabled() there ANDs the Run value with the StartupApproved flag). The
            // migrated flag is only written when the rewrite actually lands, so a
            // Task-Manager-disabled user who re-enables later still gets migrated then.
            if app_db.get_setting("autostart_args_migrated", "0").unwrap_or_default() != "1"
                && app.autolaunch().is_enabled().unwrap_or(false)
                && app.autolaunch().enable().is_ok()
            {
                let _ = app_db.set_setting("autostart_args_migrated", "1");
            }

            app.manage(app_db);
            app.manage(cache::MasterKey::new());
            app.manage(cache::CopyHandle::new());
            app.manage(cache::MirrorHandle::new());
            app.manage(cache::BackupHandle::new());
            app.manage(cache::PruneHandle::new());
            app.manage(cache::CleanupHandle::new());
            app.manage(cache::RestoreHandle::new());
            app.manage(cache::IndexHandle::new());
            app.manage(repo_locks::RepoLocks::new());

            app.manage(TrayState(Mutex::new(Tray::default())));

            // The app always starts locked (MasterKey is in-memory only), so this is always
            // the locked variant; App.tsx swaps in the unlocked one once auth succeeds.
            // Failure is logged, never fatal — a missing tray must not stop the app starting.
            if tray_on {
                match build_tray(&app.handle().clone(), false) {
                    Ok((icon, gen)) => {
                        if let Ok(mut g) = app.state::<TrayState>().0.lock() {
                            g.icon = Some(icon);
                            g.gen = gen;
                            g.unlocked = Some(false);
                        }
                    }
                    Err(e) => eprintln!("tray: failed to create at startup: {e}"),
                }
            }

            // Intercept window close: hide to tray whenever the setting is on, regardless of
            // auth state (a locked app in the tray is now a normal, supported state).
            // Otherwise close quits the app.
            if let Some(window) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                let win = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
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
                });

                if start_hidden {
                    // Accessory keeps the dock icon out of a login launch entirely.
                    // show_window() flips back to Regular whenever the window is later
                    // brought up, the same pair the close-to-tray path above uses.
                    #[cfg(target_os = "macos")]
                    let _ = app.handle().set_activation_policy(tauri::ActivationPolicy::Accessory);
                } else {
                    let _ = window.show();
                }
            }

            if start_hidden {
                // Hidden is only ever legitimate while unlocked. If we're still locked well
                // after launch, something in the frontend never got as far as reporting a
                // state (bundle error, webview crash) — surface the window so the user is
                // never left with an invisible, non-functioning app. The tray's own "Open"
                // item is the primary mitigation; this is a self-healing backstop.
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                    let still_locked = handle.state::<cache::MasterKey>().is_locked();
                    let hidden = handle
                        .get_webview_window("main")
                        .and_then(|w| w.is_visible().ok())
                        .map(|visible| !visible)
                        .unwrap_or(false);
                    if still_locked && hidden {
                        show_window(&handle);
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
                "lock_app" => { app.emit("menu:lock-app", ()).ok(); }
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
            auth::try_auto_unlock,
            auth::get_auto_unlock,
            auth::set_auto_unlock,
            auth::get_auto_unlock_supported,
            auth::auto_unlock_needs_prompt_warning,
            // repos
            repo::list_repos,
            repo::add_repo,
            repo::remove_repo,
            repo::init_repo,
            repo::rename_repo,
            repo::update_repo_path,
            repo::update_repo_read_only,
            repo::get_repo_password,
            repo::get_repo_credentials,
            repo::update_repo_secrets,
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
            repo::get_launch_at_login,
            repo::set_launch_at_login,
            repo::get_launch_at_login_warning,
            repo::get_remote_auto_refresh,
            repo::set_remote_auto_refresh,
            repo::get_auto_indexing,
            repo::set_auto_indexing,
            // notifications
            notify::get_notification_settings,
            notify::set_notification_settings,
            // webhooks
            webhook::test_webhook,
            webhook::preview_webhook,
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
            browse::cancel_restore,
            browse::index_snapshot,
            browse::index_snapshots_batch,
            browse::cancel_index_batch,
            browse::get_active_index_batch,
            browse::search_snapshot_files,
            browse::search_repo_files,
            browse::get_snapshot_index_status,
            browse::clear_snapshot_index,
            browse::get_index_progress,
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
            cache::stop_cleanup,
            cache::compress_database,
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
            show_main_window,
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

#[cfg(test)]
mod tests {
    use super::should_start_hidden;

    // Pins the "all three required" rule — see should_start_hidden's doc comment. Each of the
    // seven non-all-true combinations must stay false; only from_autostart && tray_on &&
    // auto_unlock_on may start hidden.
    #[test]
    fn should_start_hidden_requires_all_three() {
        assert!(should_start_hidden(true, true, true));
        assert!(!should_start_hidden(false, true, true));
        assert!(!should_start_hidden(true, false, true));
        assert!(!should_start_hidden(true, true, false));
        assert!(!should_start_hidden(false, false, true));
        assert!(!should_start_hidden(false, true, false));
        assert!(!should_start_hidden(true, false, false));
        assert!(!should_start_hidden(false, false, false));
    }
}
