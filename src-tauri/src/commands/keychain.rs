//! OS credential-manager access for the optional auto-unlock feature (macOS + Windows only —
//! see CLAUDE.md's Security Architecture section for the full design). Stores the *derived*
//! 32-byte master key, never the master password itself, under one fixed service/account pair.
//!
//! All four functions below are defined on every platform (Linux included) so callers — and
//! `lib.rs`'s `invoke_handler!` — stay platform-independent; the Linux bodies simply report
//! "unsupported" rather than being compiled out, avoiding the `cfg_attr(dead_code)` dance
//! `gpu_compat.rs` needs for its pure/wrapper split.

use base64::Engine;
use zeroize::Zeroize;

const SERVICE: &str = "com.nraboy.restydesktop";
const ACCOUNT: &str = "master-key";

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// keyring's own docs warn the underlying stores "may not handle access from different threads
/// reliably" (notably Windows and Linux) — every entry point below takes this first.
#[cfg(any(target_os = "macos", target_os = "windows"))]
static KEYCHAIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Result of a keychain read. Deliberately a three-way enum rather than `Result<Option<_>>` —
/// a denied macOS permission dialog must never be confused with a genuinely absent entry, or a
/// single misclick would let the caller destroy the user's auto-unlock setup. See the two
/// call sites in `auth.rs` (`try_auto_unlock`) for how each variant is handled.
pub(crate) enum LoadOutcome {
    /// Key retrieved. Still UNVERIFIED — the caller must check it against the `master_key`
    /// verification blob before trusting it (see `crypto::decrypt`).
    Found([u8; 32]),
    /// Entry genuinely absent (`keyring::Error::NoEntry`). Safe to clear the `auto_unlock`
    /// setting — there is nothing left to auto-unlock with.
    Missing,
    /// Anything else: a denied/cancelled dialog, a transient platform failure, or a corrupt
    /// stored value. Proves NOTHING about whether the stored key is actually gone or bad —
    /// callers must not delete the entry or clear the `auto_unlock` setting on this variant.
    Unreadable(String),
}

/// Decodes a stored base64 value back into a 32-byte key. Pure and platform-independent so it
/// can be unit-tested without any real keyring I/O (CI runs on ubuntu-22.04, where the store
/// doesn't exist at all).
fn decode_stored(stored: &str) -> LoadOutcome {
    let mut bytes = match B64.decode(stored) {
        Ok(b) => b,
        Err(e) => return LoadOutcome::Unreadable(e.to_string()),
    };
    if bytes.len() != 32 {
        bytes.zeroize();
        return LoadOutcome::Unreadable("stored key has the wrong length".to_string());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    bytes.zeroize();
    LoadOutcome::Found(key)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod platform {
    use super::*;
    use keyring::Entry;

    pub(super) fn is_supported() -> bool {
        true
    }

    fn entry() -> Result<Entry, String> {
        Entry::new(SERVICE, ACCOUNT).map_err(|e| e.to_string())
    }

    pub(super) fn store_key(key: &[u8; 32]) -> Result<(), String> {
        let _guard = KEYCHAIN_LOCK.lock().map_err(|e| e.to_string())?;
        let mut encoded = B64.encode(key);
        let result = entry().and_then(|e| e.set_password(&encoded).map_err(|e| e.to_string()));
        encoded.zeroize();
        result
    }

    pub(super) fn load_key() -> LoadOutcome {
        let _guard = match KEYCHAIN_LOCK.lock() {
            Ok(g) => g,
            Err(e) => return LoadOutcome::Unreadable(e.to_string()),
        };
        let e = match entry() {
            Ok(e) => e,
            Err(err) => return LoadOutcome::Unreadable(err),
        };
        match e.get_password() {
            Ok(mut stored) => {
                let outcome = decode_stored(&stored);
                stored.zeroize();
                outcome
            }
            Err(keyring::Error::NoEntry) => LoadOutcome::Missing,
            Err(err) => LoadOutcome::Unreadable(err.to_string()),
        }
    }

    pub(super) fn delete_key() -> Result<(), String> {
        let _guard = KEYCHAIN_LOCK.lock().map_err(|e| e.to_string())?;
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    pub(super) fn is_supported() -> bool {
        false
    }

    pub(super) fn store_key(_key: &[u8; 32]) -> Result<(), String> {
        Err("Not supported on this platform".to_string())
    }

    pub(super) fn load_key() -> LoadOutcome {
        LoadOutcome::Missing
    }

    pub(super) fn delete_key() -> Result<(), String> {
        Err("Not supported on this platform".to_string())
    }
}

pub(crate) fn is_supported() -> bool {
    platform::is_supported()
}

pub(crate) fn store_key(key: &[u8; 32]) -> Result<(), String> {
    platform::store_key(key)
}

pub(crate) fn load_key() -> LoadOutcome {
    platform::load_key()
}

/// Idempotent: returns `Ok(())` when there was nothing to delete, mirroring the idempotence
/// rationale already documented for `repo::set_launch_at_login`'s Windows guard — callers must
/// be able to call this unconditionally (e.g. `reset_app`) without checking existence first.
pub(crate) fn delete_key() -> Result<(), String> {
    platform::delete_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_valid_key() {
        let key = [7u8; 32];
        let encoded = B64.encode(key);
        match decode_stored(&encoded) {
            LoadOutcome::Found(k) => assert_eq!(k, key),
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn short_value_is_unreadable_not_missing() {
        let encoded = B64.encode([1u8; 31]);
        match decode_stored(&encoded) {
            LoadOutcome::Unreadable(_) => {}
            _ => panic!("expected Unreadable for a 31-byte value"),
        }
    }

    #[test]
    fn long_value_is_unreadable_not_missing() {
        let encoded = B64.encode([1u8; 33]);
        match decode_stored(&encoded) {
            LoadOutcome::Unreadable(_) => {}
            _ => panic!("expected Unreadable for a 33-byte value"),
        }
    }

    #[test]
    fn garbage_is_unreadable_not_missing() {
        match decode_stored("not-valid-base64!!!") {
            LoadOutcome::Unreadable(_) => {}
            _ => panic!("expected Unreadable for non-base64 input"),
        }
    }
}
