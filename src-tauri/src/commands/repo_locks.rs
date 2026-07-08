//! Per-repository lock registry — coordinates restic's own shared/exclusive
//! lock semantics at the app level so an exclusive-lock op (forget/prune/tag)
//! doesn't collide with a still-running shared-lock op (backup/restore/copy/
//! mirror/check/stats/snapshot-listing/indexing) against the *same* repo.
//!
//! This is purely an in-app coordination layer, not a replacement for
//! restic's own locking — it exists so an exclusive op can *wait* for the
//! repo to go idle before it starts, instead of reactively colliding with
//! restic's "repository is already locked" error (see CLAUDE.md's
//! Concurrency section for the incident that motivated this).
//!
//! Design:
//! - Keyed by repository **path** (`FullRepository.path`), which is restic's
//!   true lock identity — two `repo_id`s pointing at the same path correctly
//!   serialize against each other.
//! - Readers (`ReadGuard`) never block — they just increment a counter and
//!   return immediately. A slow writer must never make a listing/stats call
//!   hang.
//! - Writers (`WriteGuard`) poll until `readers == 0 && !exclusive`, then
//!   atomically claim the slot. **They wait genuinely until idle — no
//!   timeout, no force-claim.** An earlier version gave up after 15s and
//!   proceeded anyway ("best-effort"), which reintroduced the exact
//!   collision this registry exists to prevent whenever the op it was
//!   waiting on ran longer than 15s (confirmed in practice: a mirror running
//!   under a minute colliding with post-backup retention on the same repo).
//!   Every op that takes a `ReadGuard` today is either user-cancellable
//!   (backup/restore/copy/mirror — killing the child releases the guard
//!   immediately) or bounded by restic's own process lifetime; `check_repo`
//!   is the one op with no cancel path today, so a genuinely wedged `check`
//!   (e.g. a stalled connection to a remote repo) could make a writer on
//!   that repo wait indefinitely too — a known, narrow, pre-existing gap (a
//!   hung `check` already blocks its own modal forever, with or without this
//!   registry), not something this change introduces. See the plan doc's
//!   "Known limitations" for the full reasoning behind waiting rather than
//!   re-adding a timeout.
//! - No deadlock: readers never wait, so a writer only ever waits on readers
//!   (and at most one other writer) draining — there's no cycle. No op in
//!   this codebase acquires a read guard and then a write guard on the same
//!   path within one call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
struct RepoLockState {
    readers: u32,
    exclusive: bool,
}

impl RepoLockState {
    fn idle(&self) -> bool {
        self.readers == 0 && !self.exclusive
    }
}

/// Managed Tauri state. One entry per repo path currently in use; idle
/// entries are pruned immediately so the map stays small.
pub struct RepoLocks(Arc<Mutex<HashMap<String, RepoLockState>>>);

impl RepoLocks {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Claim a non-blocking read slot on `path`. Never waits — see the
    /// module doc for why. Drop releases it.
    pub fn read(&self, path: &str) -> ReadGuard {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(path.to_string()).or_default().readers += 1;
        ReadGuard { map: Arc::clone(&self.0), path: path.to_string() }
    }

    /// Async acquire for use in `#[tauri::command] async fn`s: polls until
    /// `path` is idle (no readers, not already exclusive), then claims it.
    /// Waits as long as it takes — see the module doc for why this doesn't
    /// time out.
    pub async fn write(&self, path: &str) -> WriteGuard {
        loop {
            if self.try_claim_exclusive(path) {
                return WriteGuard { map: Arc::clone(&self.0), path: path.to_string() };
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Sync/blocking-thread equivalent of [`write`], for call sites that
    /// aren't `async` (`apply_retention`, called from both an async command
    /// and the sync scheduler tick).
    pub fn write_blocking(&self, path: &str) -> WriteGuard {
        loop {
            if self.try_claim_exclusive(path) {
                return WriteGuard { map: Arc::clone(&self.0), path: path.to_string() };
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Atomic check-and-set: claims the exclusive slot iff the repo is
    /// currently idle. Must be a single lock acquisition — checking and
    /// setting separately would let a reader sneak in between them.
    fn try_claim_exclusive(&self, path: &str) -> bool {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let state = map.entry(path.to_string()).or_default();
        if state.idle() {
            state.exclusive = true;
            true
        } else {
            false
        }
    }
}

/// Held for the duration of a shared-lock op. Dropping decrements the
/// reader count and prunes the entry if the repo is now fully idle.
pub struct ReadGuard {
    map: Arc<Mutex<HashMap<String, RepoLockState>>>,
    path: String,
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = map.get_mut(&self.path) {
            state.readers = state.readers.saturating_sub(1);
            if state.idle() {
                map.remove(&self.path);
            }
        }
    }
}

/// Held for the duration of an exclusive-lock op (forget/prune/tag).
/// Dropping clears the exclusive flag and prunes the entry if now idle.
pub struct WriteGuard {
    map: Arc<Mutex<HashMap<String, RepoLockState>>>,
    path: String,
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = map.get_mut(&self.path) {
            state.exclusive = false;
            if state.idle() {
                map.remove(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn read_guard_increments_and_decrements() {
        let locks = RepoLocks::new();
        let g1 = locks.read("/repo");
        let g2 = locks.read("/repo");
        {
            let map = locks.0.lock().unwrap();
            assert_eq!(map.get("/repo").unwrap().readers, 2);
        }
        drop(g1);
        {
            let map = locks.0.lock().unwrap();
            assert_eq!(map.get("/repo").unwrap().readers, 1);
        }
        drop(g2);
        // Fully idle — entry pruned.
        assert!(locks.0.lock().unwrap().get("/repo").is_none());
    }

    #[tokio::test]
    async fn write_waits_for_readers_to_drain() {
        let locks = Arc::new(RepoLocks::new());
        let reader = locks.read("/repo");

        let locks2 = Arc::clone(&locks);
        let write_task = tokio::spawn(async move { locks2.write("/repo").await });

        // Give the writer a moment to start polling and confirm it hasn't
        // claimed exclusivity yet while the reader is still held.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!locks.0.lock().unwrap().get("/repo").unwrap().exclusive);

        drop(reader);
        let guard = write_task.await.unwrap();
        assert!(locks.0.lock().unwrap().get("/repo").unwrap().exclusive);
        drop(guard);
        assert!(locks.0.lock().unwrap().get("/repo").is_none());
    }

    #[tokio::test]
    async fn write_vs_write_is_mutually_exclusive() {
        let locks = Arc::new(RepoLocks::new());
        let first = locks.write("/repo").await;

        let locks2 = Arc::clone(&locks);
        let second_task = tokio::spawn(async move { locks2.write("/repo").await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        // Still exclusive-held by `first`; second must still be waiting —
        // verified indirectly by checking a manual claim attempt fails.
        assert!(!locks.try_claim_exclusive("/repo"));

        drop(first);
        let second = second_task.await.unwrap();
        assert!(locks.0.lock().unwrap().get("/repo").unwrap().exclusive);
        drop(second);
    }

    #[tokio::test]
    async fn write_waits_well_past_the_old_short_timeout_instead_of_proceeding() {
        let locks = Arc::new(RepoLocks::new());
        let reader = locks.read("/repo");

        let locks2 = Arc::clone(&locks);
        let write_task = tokio::spawn(async move { locks2.write("/repo").await });

        // The old force-claim would have proceeded at 15s; confirm the writer is still
        // waiting well past a short window and only proceeds once the reader actually drops.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!write_task.is_finished());
        assert!(!locks.0.lock().unwrap().get("/repo").unwrap().exclusive);

        drop(reader);
        let guard = write_task.await.unwrap();
        assert!(locks.0.lock().unwrap().get("/repo").unwrap().exclusive);
        drop(guard);
    }

    #[test]
    fn write_blocking_acquires_immediately_when_idle() {
        let locks = RepoLocks::new();
        let guard = locks.write_blocking("/repo");
        assert!(locks.0.lock().unwrap().get("/repo").unwrap().exclusive);
        drop(guard);
        assert!(locks.0.lock().unwrap().get("/repo").is_none());
    }

    #[test]
    fn different_paths_do_not_interfere() {
        let locks = RepoLocks::new();
        let _a = locks.read("/repo-a");
        let guard_b = locks.write_blocking("/repo-b");
        assert!(locks.0.lock().unwrap().get("/repo-b").unwrap().exclusive);
        assert_eq!(locks.0.lock().unwrap().get("/repo-a").unwrap().readers, 1);
        drop(guard_b);
    }

    #[test]
    fn reader_acquired_during_write_guard_does_not_block() {
        // Locks in the one-directional core of RepoLocks' design: a reader acquired
        // while a writer holds exclusivity must return immediately (never block, never
        // deadlock), just incrementing the reader counter. CLAUDE.md's Concurrency
        // section explicitly warns against "completing" this by making readers wait for
        // writers too; this test makes that property regression-proof. Fully synchronous
        // (write_blocking acquires instantly when idle, read never blocks) — no flakiness.
        let locks = RepoLocks::new();
        let _wg = locks.write_blocking("/repo");
        let _rg = locks.read("/repo"); // must not hang

        {
            let map = locks.0.lock().unwrap();
            let state = map.get("/repo").unwrap();
            assert_eq!(state.readers, 1, "reader should increment the reader count");
            assert!(state.exclusive, "writer should still hold exclusivity");
        }

        // Dropping the reader must NOT release the writer's claim or prune the entry.
        drop(_rg);
        {
            let map = locks.0.lock().unwrap();
            let state = map.get("/repo").unwrap();
            assert_eq!(state.readers, 0, "reader count returns to zero");
            assert!(state.exclusive, "writer still held after reader drops");
        }

        drop(_wg);
        assert!(locks.0.lock().unwrap().get("/repo").is_none(), "entry pruned once fully idle");
    }
}
