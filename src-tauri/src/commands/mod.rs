pub mod auth;
pub mod backup_plan;
pub mod browse;
pub mod cache;
pub mod crypto;
pub mod repo;
pub mod schedule;
pub mod snapshot;

use super::commands::cache::AppDb;

pub(super) fn get_restic_path(db: &AppDb) -> String {
    db.get_setting("restic_path", "restic")
        .unwrap_or_else(|_| "restic".to_string())
}

/// Prevents a console window from flashing on Windows when spawning restic.
/// On other platforms this is a no-op.
pub(super) trait NoConsole {
    fn no_console(&mut self) -> &mut Self;
    fn augment_path(&mut self) -> &mut Self;
}

impl NoConsole for std::process::Command {
    fn no_console(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        self
    }

    /// Prepend common binary locations to PATH so that a bare "restic" command
    /// resolves correctly when the app is launched from Finder or a DMG on macOS,
    /// where the inherited PATH is minimal and excludes Homebrew/MacPorts/nix paths.
    fn augment_path(&mut self) -> &mut Self {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let extra = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/opt/local/bin:/nix/var/nix/profiles/default/bin";
            let current = std::env::var("PATH").unwrap_or_default();
            let new_path = if current.is_empty() {
                extra.to_string()
            } else {
                format!("{extra}:{current}")
            };
            self.env("PATH", new_path);
        }
        self
    }
}
