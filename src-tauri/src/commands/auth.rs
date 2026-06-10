use tauri::{AppHandle, Manager, State};
use tauri_plugin_store::StoreExt;

use super::cache::{AppDb, BackupPlan, FullRepository, MasterKey};
use super::crypto;
use super::repo::run_restic_with_path;

const VERIFICATION_PLAINTEXT: &[u8] = b"restic-gui-v1-ok";

#[tauri::command]
pub fn is_app_setup(db: State<'_, AppDb>) -> Result<bool, String> {
    db.has_master_key()
}

/// Called once on first launch. Derives key, stores verification, migrates settings.json data.
#[tauri::command]
pub async fn setup_master_password(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    password: String,
) -> Result<(), String> {
    if db.has_master_key()? {
        return Err("Master password is already configured".to_string());
    }

    let salt = crypto::random_bytes::<32>();
    let key = crypto::derive_key(&password, &salt)?;
    let (nonce, ciphertext) = crypto::encrypt(&key, VERIFICATION_PLAINTEXT)?;

    db.store_master_key(&salt, &nonce, &ciphertext)?;
    master_key.set(key)?;

    migrate_from_settings_json(&app, &db, &key)?;

    Ok(())
}

/// Called on subsequent launches. Verifies password and loads key into memory.
#[tauri::command]
pub async fn unlock_app(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    password: String,
) -> Result<(), String> {
    let (salt, nonce, ciphertext) = db.load_master_key_row()?;
    let key = crypto::derive_key(&password, &salt)?;
    crypto::decrypt(&key, &nonce, &ciphertext)?;
    master_key.set(key)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = db.recalculate_overdue_schedules(now);

    // Clean up any stale locks left by a previous crash or force-quit.
    // Runs in the background so unlock_app returns immediately.
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let db = app_clone.state::<AppDb>();
        let master_key = app_clone.state::<MasterKey>();
        let key = match master_key.get() {
            Ok(k) => k,
            Err(_) => return,
        };
        let repos = match db.list_repos() {
            Ok(r) => r,
            Err(_) => return,
        };
        let restic_path = super::get_restic_path(&db);
        for repo in repos {
            if let Ok(full) = db.get_full_repo(&repo.id, &key) {
                let path = full.path.clone();
                let pass = full.password.clone();
                let rp = restic_path.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    let fr = FullRepository { path, password: pass };
                    let _ = run_restic_with_path(&fr, vec!["unlock"], &rp);
                })
                .await;
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub fn lock_app(master_key: State<'_, MasterKey>) -> Result<(), String> {
    master_key.clear()
}

/// Re-derives with a new salt, re-encrypts all passwords, updates DB.
#[tauri::command]
pub async fn change_master_password(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    let (salt, nonce, ciphertext) = db.load_master_key_row()?;
    let old_key = crypto::derive_key(&old_password, &salt)?;
    crypto::decrypt(&old_key, &nonce, &ciphertext)
        .map_err(|_| "Current master password is incorrect".to_string())?;

    let new_salt = crypto::random_bytes::<32>();
    let new_key = crypto::derive_key(&new_password, &new_salt)?;
    let (new_nonce, new_ct) = crypto::encrypt(&new_key, VERIFICATION_PLAINTEXT)?;

    db.reencrypt_repo_passwords(&old_key, &new_key)?;
    db.store_master_key(&new_salt, &new_nonce, &new_ct)?;
    master_key.set(new_key)?;
    Ok(())
}

/// Wipe all user data and return the app to first-launch state.
#[tauri::command]
pub fn reset_app(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
) -> Result<(), String> {
    db.reset_all()?;
    master_key.clear()
}

/// Read legacy settings.json and import repos, backup plans, and restic path into SQLite.
fn migrate_from_settings_json(app: &AppHandle, db: &AppDb, key: &[u8; 32]) -> Result<(), String> {
    let Ok(store) = app.store("settings.json") else {
        return Ok(());
    };

    // Migrate repos
    let repos: Vec<serde_json::Value> = store
        .get("repos")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    for repo_val in &repos {
        let id = repo_val["id"].as_str().unwrap_or_default();
        let name = repo_val["name"].as_str().unwrap_or_default();
        let path = repo_val["path"].as_str().unwrap_or_default();
        let password = repo_val["password"].as_str().unwrap_or_default();

        if id.is_empty() || path.is_empty() {
            continue;
        }
        let (nonce, ciphertext) = crypto::encrypt(key, password.as_bytes())?;
        db.add_repo(id, name, path, &nonce, &ciphertext)?;
    }

    // Migrate backup plans
    let plans: Vec<BackupPlan> = store
        .get("backup_plans")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    for plan in &plans {
        db.save_backup_plan(plan)?;
    }

    // Migrate restic path
    if let Some(restic_path) = store.get("restic_path").and_then(|v| v.as_str().map(str::to_string)) {
        db.set_setting("restic_path", &restic_path)?;
    }

    Ok(())
}
