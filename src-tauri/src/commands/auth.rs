use tauri::{AppHandle, Manager, State};

use super::cache::{AppDb, MasterKey};
use super::crypto;
use super::keychain;
use super::repo::{set_launch_at_login, unlock_quietly};

const VERIFICATION_PLAINTEXT: &[u8] = b"restic-gui-v1-ok";

/// Mirrors the client-side check both `AuthPage.tsx` (setup) and `SettingsPage.tsx` (rotation)
/// already enforce. The backend previously enforced nothing here — `derive_key("")` succeeds —
/// while `validate_init_password` (repo.rs) already rejects an empty *repo* password, leaving
/// the master password inconsistent with its own repo-password sibling. Defense in depth only:
/// every client already blocks a short password before this ever runs.
const MIN_MASTER_PASSWORD_LEN: usize = 8;

/// Pure — no DB/Tauri state — so it's directly unit-testable, matching
/// `validate_init_password`'s (repo.rs) shape.
pub(crate) fn validate_master_password(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_MASTER_PASSWORD_LEN {
        return Err(format!("Master password must be at least {MIN_MASTER_PASSWORD_LEN} characters."));
    }
    Ok(())
}

#[tauri::command]
pub fn is_app_setup(db: State<'_, AppDb>) -> Result<bool, String> {
    db.has_master_key()
}

/// Called once on first launch. Derives key and stores the verification blob.
#[tauri::command]
pub async fn setup_master_password(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    password: String,
) -> Result<(), String> {
    if db.has_master_key()? {
        return Err("Master password is already configured".to_string());
    }
    validate_master_password(&password)?;

    let salt = crypto::random_bytes::<32>();
    let key = crypto::derive_key(&password, &salt)?;
    let (nonce, ciphertext) = crypto::encrypt(&key, VERIFICATION_PLAINTEXT)?;

    db.store_master_key(&salt, &nonce, &ciphertext)?;
    master_key.set(key)?;

    Ok(())
}

/// Cleans up any stale restic locks left by a previous crash or force-quit. Runs in the
/// background so the caller (`unlock_app`, `try_auto_unlock`) returns immediately. Shared by
/// both entry points to the unlocked state — see each call site's own comment.
pub(crate) fn spawn_stale_lock_cleanup(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let db = app.state::<AppDb>();
        let master_key = app.state::<MasterKey>();
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
            // A read-only repo is opened with --no-lock (see repo::apply_repo_flags), so it
            // never took a lock in the first place, and `restic unlock` would just fail
            // against its read-only backing store. Nothing to clean up.
            if repo.read_only {
                continue;
            }
            if let Ok(full) = db.get_full_repo(&repo.id, &key) {
                let rp = restic_path.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    unlock_quietly(&full, &rp);
                })
                .await;
            }
        }
    });
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

    spawn_stale_lock_cleanup(app);

    Ok(())
}

#[tauri::command]
pub fn lock_app(master_key: State<'_, MasterKey>) -> Result<(), String> {
    master_key.clear()
}

/// Re-derives with a new salt, re-encrypts all passwords, updates DB.
#[tauri::command]
pub async fn change_master_password(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    old_password: String,
    new_password: String,
) -> Result<(), String> {
    let (salt, nonce, ciphertext) = db.load_master_key_row()?;
    let old_key = crypto::derive_key(&old_password, &salt)?;
    crypto::decrypt(&old_key, &nonce, &ciphertext)
        .map_err(|_| "Current master password is incorrect".to_string())?;

    // Checked only after the old-password verification above, deliberately: validating first
    // would let a caller probe password policy without knowing the current password, and would
    // reject a legitimate rotation attempt before telling the user their current password was
    // wrong instead of after.
    validate_master_password(&new_password)?;

    let new_salt = crypto::random_bytes::<32>();
    let new_key = crypto::derive_key(&new_password, &new_salt)?;
    let (new_nonce, new_ct) = crypto::encrypt(&new_key, VERIFICATION_PLAINTEXT)?;

    db.rotate_master_key(&old_key, &new_key, &new_salt, &new_nonce, &new_ct)?;
    master_key.set(new_key)?;

    // Rotation derives a brand new key against a brand new salt, so a stored auto-unlock key
    // (if any) is now stale and must be replaced. This runs strictly after the rotation above
    // commits, so a keychain failure here can never affect the database. On failure, the entry
    // and the `auto_unlock` row are deliberately left as-is rather than cleared — the failure
    // may just be a denied macOS dialog, which proves nothing about the key, and clearing on
    // that would silently disable auto-unlock from one misclick (see keychain.rs's LoadOutcome
    // doc comment for the same principle). If the stored key really is stale, the next launch's
    // verification step in try_auto_unlock self-heals it.
    if db.get_setting("auto_unlock", "false")? == "true" {
        let key_to_store = new_key;
        let store_result = tauri::async_runtime::spawn_blocking(move || keychain::store_key(&key_to_store))
            .await
            .map_err(|e| e.to_string());
        match store_result {
            Ok(Ok(())) => {
                db.set_setting("auto_unlock_last_version", &app.package_info().version.to_string())?;
            }
            Ok(Err(e)) | Err(e) => {
                // Best-effort only — the password rotation above already committed, and this
                // command's contract must not turn a successful password change into a
                // reported failure (SettingsPage.tsx's handleChangePassword treats any Err as
                // "nothing happened," true for every other command here, so it must stay true
                // for this one too). The stale keychain entry self-heals on its own: the next
                // try_auto_unlock attempt will fail verification against it (old key vs. new
                // ciphertext), delete it, clear the auto_unlock row, and show the existing
                // "stale" notice on the unlock screen — the same recovery path a denied dialog
                // already takes.
                eprintln!(
                    "change_master_password: could not refresh the auto-unlock key ({e}); it will self-heal on next launch"
                );
            }
        }
    }

    Ok(())
}

/// Wipe all user data and return the app to first-launch state.
#[tauri::command]
pub fn reset_app(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
) -> Result<(), String> {
    db.reset_all()?;
    // Best-effort: the autostart entry lives in the OS (LaunchAgent plist / HKCU Run value /
    // XDG .desktop), not in app_settings, so reset_all can't reach it. Left behind, the machine
    // would keep launching the app at login with no way to turn it off from Settings — the
    // toggle renders off once tray_enabled reverts to its false default. Failure here must not
    // fail the reset: wiping user data is the part that matters, and set_launch_at_login is
    // already idempotent (see its Windows guard).
    let _ = set_launch_at_login(app, false);
    // Same reasoning: the auto-unlock key lives outside the DB, so reset_all can't reach it
    // either. Left behind, a reset app would still auto-unlock itself into a freshly-wiped,
    // now-mismatched database on the next launch. delete_key is idempotent (Ok on NoEntry).
    let _ = keychain::delete_key();
    master_key.clear()
}

/// Result of an auto-unlock attempt, returned to the frontend so it can explain *why* the user
/// landed on the manual unlock screen rather than showing a bare failure. `reason` is a
/// machine-readable code, not display text — AuthPage.tsx owns the actual copy.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoUnlockResult {
    pub unlocked: bool,
    pub reason: String,
}

/// UI state only for the `auto_unlock` toggle — see keychain.rs's `LoadOutcome` doc comment and
/// CLAUDE.md's Security Architecture for why this deliberately diverges from launch-at-login's
/// "no app_settings row" rule (documented under Intentional Designs). A keychain read is neither
/// cheap nor silent — on macOS it can raise a permission dialog — so deriving this toggle's
/// *display* state from the keychain itself would prompt every time Settings mounts. This row
/// never claims a state the keychain doesn't back: see `set_auto_unlock`'s ordering and
/// `try_auto_unlock`'s self-healing paths below.
#[tauri::command]
pub fn get_auto_unlock(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("auto_unlock", "false")? == "true")
}

/// Whether this platform can store a secret at all. macOS/Windows only — see keychain.rs.
#[tauri::command]
pub fn get_auto_unlock_supported() -> bool {
    keychain::is_supported()
}

/// Enables or disables storing the (derived) master key in the OS credential manager.
///
/// Enabling stores the key *before* writing the `auto_unlock` row, so a failed store never
/// leaves the row claiming a state the keychain doesn't back. Disabling deletes the entry
/// *before* clearing the row, but — unlike the enable path — the row is cleared even if the
/// delete itself failed: the key must stop being used regardless, though the caller is told the
/// delete didn't actually succeed rather than being falsely reassured the secret is gone.
#[tauri::command]
pub async fn set_auto_unlock(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    value: bool,
) -> Result<(), String> {
    if value {
        let key = master_key.get()?;
        tauri::async_runtime::spawn_blocking(move || keychain::store_key(&key))
            .await
            .map_err(|e| e.to_string())??;
        db.set_setting("auto_unlock", "true")?;
        db.set_setting("auto_unlock_last_version", &app.package_info().version.to_string())?;
        Ok(())
    } else {
        let delete_result = tauri::async_runtime::spawn_blocking(keychain::delete_key)
            .await
            .map_err(|e| e.to_string());
        db.set_setting("auto_unlock", "false")?;
        match delete_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) | Err(e) => Err(format!(
                "Auto-unlock has been turned off, but the saved key could not be removed from your credential manager ({e}). You may want to remove it manually."
            )),
        }
    }
}

/// Whether the macOS "wants to use your confidential information" dialog is expected on this
/// launch — true only when auto-unlock is on, the platform is macOS, and the app version has
/// changed since the last successful keychain read/write (see the module-level design note in
/// the plan: ad-hoc code signing means the keychain ACL's designated requirement changes on
/// every rebuild). Always false on Windows, which never prompts, and false whenever auto-unlock
/// is off, so a user who has never enabled the feature never sees this check do anything.
#[tauri::command]
pub fn auto_unlock_needs_prompt_warning(app: AppHandle, db: State<'_, AppDb>) -> Result<bool, String> {
    if !cfg!(target_os = "macos") {
        return Ok(false);
    }
    if db.get_setting("auto_unlock", "false")? != "true" {
        return Ok(false);
    }
    let last_version = db.get_setting("auto_unlock_last_version", "")?;
    Ok(last_version != app.package_info().version.to_string())
}

/// Attempts to unlock using a key previously stored via `set_auto_unlock`. Called once at
/// startup, before the frontend shows the unlock screen. Every failure path returns
/// `Ok(AutoUnlockResult { unlocked: false, .. })` rather than `Err` — auto-unlock failing is an
/// expected, routine outcome (the toggle is off, the entry is missing, the user denied the
/// platform dialog), not an application error.
#[tauri::command]
pub async fn try_auto_unlock(
    app: AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
) -> Result<AutoUnlockResult, String> {
    // Step 1: the keychain is never touched unless this row is "true". This ordering is what
    // guarantees a user who has never enabled the feature never sees a macOS permission dialog.
    if db.get_setting("auto_unlock", "false")? != "true" {
        return Ok(AutoUnlockResult { unlocked: false, reason: String::new() });
    }

    // Step 2: a macOS read can block on a modal dialog for as long as the user ignores it, so
    // this must run off the async runtime (see CLAUDE.md's Persistence & Caching rule against
    // blocking a core worker thread).
    let outcome = tauri::async_runtime::spawn_blocking(keychain::load_key)
        .await
        .map_err(|e| e.to_string())?;

    let key = match outcome {
        keychain::LoadOutcome::Missing => {
            // Genuinely absent — safe to clear the row, nothing left to auto-unlock with.
            db.set_setting("auto_unlock", "false")?;
            return Ok(AutoUnlockResult { unlocked: false, reason: String::new() });
        }
        keychain::LoadOutcome::Unreadable(err) => {
            // Proves nothing about the stored key — could be a denied dialog, a cancelled
            // prompt, or a transient platform failure. Deliberately change nothing here; the
            // entry and the row are left exactly as they were, and the next launch retries.
            // The underlying error is only useful for local troubleshooting (it's never
            // surfaced to the user — "denied" is the only thing the frontend sees).
            eprintln!("auto-unlock: keychain read failed, leaving stored key untouched: {err}");
            return Ok(AutoUnlockResult { unlocked: false, reason: "denied".to_string() });
        }
        keychain::LoadOutcome::Found(key) => key,
    };

    // Step 3: verify before trusting — the same check unlock_app performs, just against a
    // keychain-sourced key instead of one freshly derived from the typed password.
    let (_salt, nonce, ciphertext) = db.load_master_key_row()?;
    if crypto::decrypt(&key, &nonce, &ciphertext).is_err() {
        // The stored key no longer matches this database (e.g. the app was reset, or the
        // database was restored from another machine). Self-heal: delete the stale entry and
        // clear the row so the user isn't stuck retrying a key that can never work again.
        let _ = tauri::async_runtime::spawn_blocking(keychain::delete_key).await;
        db.set_setting("auto_unlock", "false")?;
        return Ok(AutoUnlockResult { unlocked: false, reason: "stale".to_string() });
    }

    // Only record a successful read as "seen this version" — if the user had instead denied
    // the dialog, the marker stays stale so auto_unlock_needs_prompt_warning fires again next
    // launch, when they'll actually be prompted again.
    db.set_setting("auto_unlock_last_version", &app.package_info().version.to_string())?;

    master_key.set(key)?;
    spawn_stale_lock_cleanup(app);

    Ok(AutoUnlockResult { unlocked: true, reason: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_master_password_rejects_empty() {
        assert!(validate_master_password("").is_err());
    }

    #[test]
    fn validate_master_password_rejects_short() {
        assert!(validate_master_password("short1").is_err());
        assert!(validate_master_password("1234567").is_err()); // 7 chars
    }

    #[test]
    fn validate_master_password_accepts_min_length() {
        assert!(validate_master_password("12345678").is_ok()); // exactly 8
    }

    #[test]
    fn validate_master_password_accepts_longer() {
        assert!(validate_master_password("a reasonably long passphrase").is_ok());
    }
}
