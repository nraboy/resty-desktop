pub mod auth;
pub mod backup_plan;
pub mod browse;
pub mod cache;
pub mod crypto;
pub mod repo;
pub mod snapshot;

use super::commands::cache::AppDb;

pub(super) fn get_restic_path(db: &AppDb) -> String {
    db.get_setting("restic_path", "restic")
        .unwrap_or_else(|_| "restic".to_string())
}
