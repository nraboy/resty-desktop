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
}
