//! OS credential-manager access for the optional auto-unlock feature (see CLAUDE.md's Security
//! Architecture section for the full design). Stores the *derived* 32-byte master key, never the
//! master password itself, under one fixed service/account pair.
//!
//! Backed by `keyring`'s `v1` compatibility API, which selects the platform store itself: the
//! macOS Keychain (the legacy `keychain` store, not the data-protection one), the Windows
//! Credential Manager, and the D-Bus Secret Service on Linux via pure-Rust zbus (no libdbus).
//! Linux support is explicitly *best-effort* — the store may be absent or locked at startup, in
//! which case every path below degrades to the master-password prompt rather than failing. See
//! docs/decisions.md.
//!
//! All four functions below are defined on every platform so callers — and `lib.rs`'s
//! `invoke_handler!` — stay platform-independent; a target with no supported store (i.e. none of
//! macOS/Windows/Linux) falls through to a stub `mod platform` that reports "unsupported".

use base64::Engine;
use zeroize::Zeroize;

const SERVICE: &str = "com.nraboy.restydesktop";
const ACCOUNT: &str = "master-key";

const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// keyring's own docs warn the underlying stores "may not handle access from different threads
/// reliably" (notably Windows and Linux, where Linux's D-Bus-based store adds RPC ordering on
/// top) — every entry point below takes this first.
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
static KEYCHAIN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Result of a keychain read. Deliberately a three-way enum rather than `Result<Option<_>>` —
/// a denied macOS permission dialog (or a locked Linux Secret Service collection) must never be
/// confused with a genuinely absent entry, or a single misclick/locked-session could let the
/// caller destroy the user's auto-unlock setup. See the two call sites in `auth.rs`
/// (`try_auto_unlock`) for how each variant is handled.
pub(crate) enum LoadOutcome {
    /// Key retrieved. Still UNVERIFIED — the caller must check it against the `master_key`
    /// verification blob before trusting it (see `crypto::decrypt`).
    Found([u8; 32]),
    /// Entry genuinely absent (`keyring::Error::NoEntry`). Safe to clear the `auto_unlock`
    /// setting — there is nothing left to auto-unlock with.
    Missing,
    /// Anything else: a denied/cancelled dialog, a locked Linux collection, a transient platform
    /// failure, or a corrupt stored value. Proves NOTHING about whether the stored key is
    /// actually gone or bad — callers must not delete the entry or clear the `auto_unlock`
    /// setting on this variant.
    Unreadable(String),
}

/// Decodes a stored base64 value back into a 32-byte key. Pure and platform-independent so it
/// can be unit-tested without any real keyring I/O (CI runs on ubuntu-22.04, which has no
/// session bus, so the store itself is unreachable there regardless).
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

/// Pure form of the Linux session-bus probe — see `linux_session_bus_available`.
#[cfg(target_os = "linux")]
fn session_bus_present(dbus_addr: Option<&std::ffi::OsStr>, runtime_bus_exists: bool) -> bool {
    if let Some(addr) = dbus_addr {
        if !addr.is_empty() {
            return true;
        }
    }
    runtime_bus_exists
}

/// Whether this Linux session looks like it has a D-Bus session bus, and therefore could have a
/// Secret Service provider. Deliberately environment/filesystem inspection only, never a D-Bus
/// round trip: `is_supported()` backs `get_auto_unlock_supported`, which `SettingsPage` calls on
/// every mount, so it must stay silent and cheap (same reasoning as the `auto_unlock` row's own
/// existence — see `auth.rs::get_auto_unlock`'s doc comment). A `true` result is *not* a promise
/// that a Secret Service provider actually answers on that bus — it only decides whether the
/// toggle is worth showing at all. The real verification is `set_auto_unlock`'s store-before-write
/// (`auth.rs::set_auto_unlock`), which leaves the toggle off and surfaces the error to the user if
/// nothing answers.
#[cfg(target_os = "linux")]
fn linux_session_bus_available() -> bool {
    let addr = std::env::var_os("DBUS_SESSION_BUS_ADDRESS");
    let runtime_bus_exists = std::env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| std::path::Path::new(&dir).join("bus").exists())
        .unwrap_or(false);
    session_bus_present(addr.as_deref(), runtime_bus_exists)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod platform {
    use super::*;
    use keyring::Entry;

    pub(super) fn is_supported() -> bool {
        #[cfg(target_os = "linux")]
        {
            super::linux_session_bus_available()
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
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

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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

// Deliberately does not test `linux_session_bus_available()` or `is_supported()` directly —
// GitHub's ubuntu-22.04 runners have no session bus, so the ambient environment varies and an
// assertion on either return value would be flaky. `session_bus_present` is the pure core; it
// takes its inputs as arguments so this stays a real unit test rather than a probe of CI's box.
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::session_bus_present;
    use std::ffi::OsStr;

    #[test]
    fn a_session_bus_address_is_enough() {
        assert!(session_bus_present(Some(OsStr::new("unix:path=/run/user/1000/bus")), false));
    }

    #[test]
    fn an_empty_address_falls_back_to_the_runtime_socket() {
        assert!(!session_bus_present(Some(OsStr::new("")), false));
        assert!(session_bus_present(Some(OsStr::new("")), true));
    }

    #[test]
    fn neither_address_nor_socket_means_unsupported() {
        assert!(!session_bus_present(None, false));
        assert!(session_bus_present(None, true));
    }
}
