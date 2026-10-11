//! Explore: the repository as one folder tree, with no snapshot dimension.
//!
//! The queries themselves live in `cache.rs` beside the rest of the persistence layer —
//! `AppDb`'s connection is private to that module and Rust privacy is module-and-descendants
//! rather than sibling, so a query here could not reach it. This module holds the commands
//! and their pure helpers, following the same split `search_repo_files` already uses.

use tauri::Manager;

use super::browse::RepoFileHit;
use super::cache::AppDb;

/// Collapses the folder arriving from the page's `?path=` parameter into the form
/// `browse_cache_files.parent_path` actually stores.
///
/// `AppDb::list_repo_children` matches that column by equality (which is what lets it ride
/// the existing `(snap, parent_path)` index), so the spellings `list_files`'s
/// `is_direct_child` treats interchangeably have to be made identical here instead: absent,
/// empty and a bare "/" all mean the top level, which is stored as `""`. A trailing
/// separator is trimmed. The *leading* separator is deliberately kept — stored paths carry
/// it, so stripping it would turn every nested folder into a miss.
///
/// Without this, a root path restored from browser history or typed in as "/" would return
/// nothing and present as an empty repository rather than its top level.
pub(crate) fn normalize_parent(path: Option<&str>) -> String {
    match path {
        None => String::new(),
        Some(p) => {
            let trimmed = p.trim_end_matches('/');
            if trimmed.is_empty() {
                String::new()
            } else {
                trimmed.to_string()
            }
        }
    }
}

/// Lists one folder of the repository's unified tree: the direct children of `path`,
/// each path once, resolved to the newest fully-indexed snapshot holding it.
///
/// A pure database read, so it follows the settled treatment of `list_snapshots` /
/// `get_repo_stats` — no `RepoLocks` guard and nothing emitted on the `task` bus — and it
/// never shells out to restic. The database work runs on a blocking thread so the shared
/// `AppDb` connection mutex is never held on an async runtime worker.
///
/// Returns rows only. Index coverage is derived on the frontend from the repo's snapshot
/// list and its per-snapshot index statuses, not folded into this result.
#[tauri::command]
pub async fn list_repo_children(
    app: tauri::AppHandle,
    repo_id: String,
    path: Option<String>,
) -> Result<Vec<RepoFileHit>, String> {
    let parent = normalize_parent(path.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        db.list_repo_children(&repo_id, &parent)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_parent_collapses_every_root_form_to_the_stored_empty_string() {
        // Top-level cache rows are stored with `parent_path = ""`, and the folder reaches
        // the query by equality, so each way of spelling "the root" must arrive as "".
        assert_eq!(normalize_parent(None), "");
        assert_eq!(normalize_parent(Some("")), "");
        assert_eq!(normalize_parent(Some("/")), "");
    }

    #[test]
    fn normalize_parent_trims_a_trailing_separator_but_keeps_the_leading_one() {
        // Stored paths carry their leading slash, so only the trailing one is noise —
        // typically from a hand-edited or history-restored address.
        assert_eq!(normalize_parent(Some("/home/docs")), "/home/docs");
        assert_eq!(normalize_parent(Some("/home/docs/")), "/home/docs");
    }
}
