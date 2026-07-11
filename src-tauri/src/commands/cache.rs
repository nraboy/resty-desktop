use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::str::FromStr;

use chrono::Local;
use cron::Schedule as CronSchedule;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::browse::FileEntry;
use super::crypto;
use super::snapshot::Snapshot;
use crate::tasks::{new_task_slot, TaskSlot};

/// Max rows retained in `backup_history`. Read and trim both use this so they
/// never drift — the Logs page never shows rows the trim would have deleted.
const BACKUP_HISTORY_LIMIT: i64 = 1000;

/// (salt, verification_nonce, verification_ciphertext) from the `master_key` table.
type MasterKeyRow = (Vec<u8>, Vec<u8>, Vec<u8>);

// ── public types (serialised to frontend) ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupHistoryEntry {
    pub id: String,
    pub repo_id: String,
    pub repo_name: Option<String>,
    pub plan_id: Option<String>,
    pub plan_name: Option<String>,
    pub snapshot_id: Option<String>,
    pub started_at: i64,
    pub duration_seconds: f64,
    pub files_new: u64,
    pub files_changed: u64,
    pub bytes_added: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    pub keep_last: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_yearly: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPlan {
    pub id: String,
    pub name: String,
    pub repo_id: String,
    pub paths: Vec<String>,
    pub tags: Vec<String>,
    pub excludes: Vec<String>,
    pub retention: Option<RetentionPolicy>,
    pub limit_upload: Option<u32>,
    pub limit_download: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub plan_ids: Vec<String>,
    pub cron_expr: String,
    pub enabled: bool,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub created_at: i64,
}

// ── internal type (never serialised) ───────────────────────────────────────

#[derive(Clone, ZeroizeOnDrop)]
pub struct FullRepository {
    #[zeroize(skip)]
    pub path: String,
    pub password: String,
}

// ── copy cancellation handle ──────────────────────────────────────────────

pub struct CopyHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a copy is executing. Serializes copies so two concurrent
    /// `copy_snapshot` calls can't corrupt the shared `child`/`cancelled`
    /// state (matches the pattern already used by BackupHandle/RestoreHandle).
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `cancel_copy` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl CopyHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
            current_task: new_task_slot(),
        }
    }
}

pub struct MirrorHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a mirror is executing. Serializes mirrors so two concurrent
    /// `mirror_repo` calls can't corrupt the shared `child`/`cancelled` state.
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `cancel_mirror` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl MirrorHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
            current_task: new_task_slot(),
        }
    }
}

pub struct BackupHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a backup is executing. Serializes backups so two concurrent
    /// `execute_backup` calls (e.g. a scheduler tick colliding with a manual
    /// backup) can't corrupt the shared `child`/`cancelled` state.
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `cancel_backup` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl BackupHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
            current_task: new_task_slot(),
        }
    }
}

pub struct PruneHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a prune is executing. Serializes prune_repo/prune_all_repos —
    /// they previously shared this handle with no serialization, so a
    /// concurrent second run could clobber the first run's `child`/`cancelled`
    /// state (a second Stop could kill the wrong process, or vice versa).
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `cancel_prune` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl PruneHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
            current_task: new_task_slot(),
        }
    }
}

pub struct RestoreHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a restore is executing. Serializes restores so two concurrent
    /// `restore_snapshot` calls (e.g. the user starting a restore on one repo,
    /// navigating away, then starting another) can't corrupt the shared
    /// `child`/`cancelled` state or let Stop kill the wrong process.
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `cancel_restore` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl RestoreHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
            current_task: new_task_slot(),
        }
    }
}

/// Coordinates manual (user-triggered) snapshot indexing with the background
/// cache_warmer auto-indexer so at most one `run_full_index` ever runs at a
/// time, bounding memory to a single snapshot's file list.
pub struct IndexHandle {
    /// Reference count of manual indexing runs currently active (single-snapshot
    /// or batch, including a batch that's merely *queued* on `batch_turn` — see
    /// its doc comment below). The cache_warmer sweep checks `!= 0` to avoid
    /// starting new auto-indexing work while any manual indexing is active or
    /// pending. A plain bool doesn't work here: a queued batch and the batch
    /// ahead of it in `batch_turn` both hold a `ManualIndexGuard` at once, and a
    /// bool would let the front batch's `Drop` clear the flag out from under the
    /// still-waiting one, letting the warmer slip an auto-index in between
    /// batches. Incremented/decremented by `ManualIndexGuard`.
    pub manual_active: Arc<std::sync::atomic::AtomicUsize>,
    /// Acquired around every `run_full_index` call, in both the manual and
    /// auto-indexer paths, held across the `spawn_blocking(...).await`. Closes
    /// the race where the auto sweep is already mid-index when manual
    /// indexing starts — guarantees strictly one indexing process at a time.
    /// Legitimately global (unlike `batches` below): this bounds how many
    /// `restic` processes run concurrently, not which logical batch owns them.
    pub gate: Arc<tokio::sync::Mutex<()>>,
    /// Acquired once per "Index All" batch, held for the *entire* batch (all its
    /// snapshots), so concurrent batches (e.g. for different repos) complete in
    /// start order instead of round-robin-interleaving their snapshots against
    /// each other. Distinct from `gate`: `gate` bounds how many `restic`
    /// processes run at once (still taken/released per-snapshot inside the
    /// running batch, so a single `index_snapshot` or the auto-indexer can still
    /// slip in between that batch's snapshots); `batch_turn` only orders whole
    /// batches against each other. tokio's `Mutex` is FIFO among waiters, so
    /// batches complete in (approximately, since each is an independently
    /// spawned task) the order they started — sufficient for human-paced
    /// clicks. A batch waiting on this is "queued"; see `BatchCancel::cancel_notify`
    /// for how a queued batch still cancels promptly instead of waiting its turn.
    pub batch_turn: Arc<tokio::sync::Mutex<()>>,
    /// Per-batch cancel flag + task slot, keyed by operationId, so concurrent
    /// "Index All" batches (e.g. different repos running at once) can be
    /// cancelled independently instead of sharing one flag/slot across every
    /// batch — a prior single-shared-field design meant starting a second
    /// batch could silently steal the first's cancel target, and clicking Stop
    /// on one batch could kill both. Populated by `index_snapshots_batch` when
    /// a batch starts, removed when it reaches a terminal state (see
    /// `BatchDeregisterGuard` in browse.rs).
    pub batches: Arc<Mutex<HashMap<String, BatchCancel>>>,
}

/// One batch's cancel flag + task slot, registered in `IndexHandle::batches` for the
/// duration of an `index_snapshots_batch` run. `cancel_index_batch` looks this up by
/// operationId so it can target exactly one running batch.
#[derive(Clone)]
pub struct BatchCancel {
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub task_slot: TaskSlot,
    /// Wakes a batch that's parked waiting for its turn on `IndexHandle::batch_turn`
    /// so it can cancel immediately instead of waiting for the batch ahead of it to
    /// finish. `cancel_index_batch` calls `notify_one()` right after setting `cancel`;
    /// the batch's `tokio::select!` between this and `batch_turn.lock()` picks up
    /// whichever fires first. `notify_one`'s stored-permit semantics mean this is
    /// race-free even if the notify arrives before the batch starts waiting.
    pub cancel_notify: Arc<tokio::sync::Notify>,
    /// False while the batch is still queued waiting for `IndexHandle::batch_turn`
    /// (registered but not yet `activate()`d); flipped true the moment it wins its
    /// turn and starts actually indexing. Lets `get_active_index_batch` (browse.rs)
    /// report queued-vs-running to a frontend that just (re)mounted and missed the
    /// live `pending`/`started` task events, without needing to inspect `task_slot`
    /// (which only carries identity, not lifecycle phase).
    pub started: Arc<std::sync::atomic::AtomicBool>,
    /// The batch's full snapshot-id target list, fixed at creation and never mutated.
    /// Lets `get_active_index_batch` (browse.rs) hand a page that just (re)mounted the
    /// *exact* set of snapshots this batch is indexing, so it can restore accurate local
    /// progress state (which of these are already done, per the index-status cache it
    /// already has) instead of only knowing "a batch exists" with no way to know what
    /// it's actually working on.
    pub target_ids: Arc<Vec<String>>,
}

impl IndexHandle {
    pub fn new() -> Self {
        Self {
            manual_active: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            batch_turn: Arc::new(tokio::sync::Mutex::new(())),
            batches: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// ── in-memory master-key state ─────────────────────────────────────────────

pub struct MasterKey(pub Mutex<Option<[u8; 32]>>);

impl MasterKey {
    pub fn new() -> Self {
        MasterKey(Mutex::new(None))
    }

    pub fn get(&self) -> Result<[u8; 32], String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "App is locked — please unlock first".to_string())
    }

    pub fn set(&self, key: [u8; 32]) -> Result<(), String> {
        let mut guard = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(mut old) = guard.replace(key) {
            old.zeroize();
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        let mut guard = self.0.lock().map_err(|e| e.to_string())?;
        if let Some(mut key) = guard.take() {
            key.zeroize();
        }
        Ok(())
    }
}

// ── database ───────────────────────────────────────────────────────────────

pub struct AppDb {
    conn: Mutex<Connection>,
    db_path: std::path::PathBuf,
}

impl AppDb {
    pub fn new(conn: Connection, db_path: std::path::PathBuf) -> Self {
        Self {
            conn: Mutex::new(conn),
            db_path,
        }
    }

    pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
        // No-op on a database that already has tables — page_size can only be
        // changed on an empty database (or via VACUUM). Harmless to attempt
        // unconditionally on every launch.
        let _ = conn.execute_batch("PRAGMA page_size = 8192;");

        // v0 → v1: replace JSON-blob browse_cache and snapshots_cache with relational tables.
        // Cache loss is safe — the app falls back to live restic fetches.
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(
                "DROP TABLE IF EXISTS browse_cache;
                 DROP TABLE IF EXISTS snapshots_cache;
                 PRAGMA user_version = 1;",
            )?;
        }
        if version < 2 {
            // browse_cache_files/status schema changed (snapshot_id interned to
            // an integer, name/per-row cached_at dropped) — both tables are a
            // disposable cache rebuildable via restic ls, so just drop and let
            // re-indexing repopulate.
            conn.execute_batch(
                "DROP TABLE IF EXISTS browse_cache_files;
                 DROP TABLE IF EXISTS browse_cache_status;
                 PRAGMA user_version = 2;",
            )?;
            // DROP TABLE moves pages to SQLite's freelist; the data is gone.
            // We deliberately do NOT VACUUM here — doing so on the main thread
            // would block window creation for an O(file-size) rewrite on upgrade.
            // The freelist pages are reused in place as the cache rebuilds via
            // re-indexing (no doubling), and users who want to shrink the file
            // can use "Clear All Cache", which already does its own VACUUM.
        }

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA busy_timeout=5000;
            CREATE TABLE IF NOT EXISTS master_key (
                id                      INTEGER PRIMARY KEY CHECK (id = 1),
                salt                    BLOB NOT NULL,
                verification_nonce      BLOB NOT NULL,
                verification_ciphertext BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS repositories (
                id                  TEXT PRIMARY KEY,
                name                TEXT NOT NULL,
                path                TEXT NOT NULL,
                password_nonce      BLOB NOT NULL,
                password_ciphertext BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS backup_plans (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                repo_id         TEXT NOT NULL,
                paths_json      TEXT NOT NULL,
                tags_json       TEXT NOT NULL,
                excludes_json   TEXT NOT NULL,
                retention_json  TEXT,
                limit_upload    INTEGER,
                limit_download  INTEGER
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS indexed_snapshots (
                id           INTEGER PRIMARY KEY,
                snapshot_id  TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS browse_cache_files (
                snap         INTEGER NOT NULL,
                path         TEXT NOT NULL,
                parent_path  TEXT NOT NULL,
                entry_type   TEXT NOT NULL,
                size         INTEGER,
                mtime        TEXT,
                mode         INTEGER,
                PRIMARY KEY (snap, path)
            );
            CREATE INDEX IF NOT EXISTS idx_browse_files
                ON browse_cache_files (snap, parent_path);
            CREATE TABLE IF NOT EXISTS browse_cache_status (
                repo_id      TEXT NOT NULL,
                snapshot_id  TEXT NOT NULL,
                status       TEXT NOT NULL DEFAULT 'pending',
                cached_at    INTEGER,
                PRIMARY KEY (repo_id, snapshot_id)
            );
            CREATE TABLE IF NOT EXISTS snapshots_cache (
                repo_id      TEXT NOT NULL,
                snapshot_id  TEXT NOT NULL,
                short_id     TEXT NOT NULL,
                time         TEXT NOT NULL,
                hostname     TEXT NOT NULL,
                username     TEXT,
                paths        TEXT NOT NULL,
                tags         TEXT,
                cached_at    INTEGER NOT NULL,
                PRIMARY KEY (repo_id, snapshot_id)
            );
            CREATE TABLE IF NOT EXISTS repo_stats_cache (
                repo_id          TEXT PRIMARY KEY,
                total_size       INTEGER NOT NULL,
                total_file_count INTEGER NOT NULL,
                snapshots_count  INTEGER NOT NULL,
                cached_at        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS backup_history (
                id               TEXT PRIMARY KEY,
                repo_id          TEXT NOT NULL,
                plan_id          TEXT,
                snapshot_id      TEXT,
                started_at       INTEGER NOT NULL,
                duration_seconds REAL NOT NULL,
                files_new        INTEGER NOT NULL DEFAULT 0,
                files_changed    INTEGER NOT NULL DEFAULT 0,
                bytes_added      INTEGER NOT NULL DEFAULT 0,
                error            TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_history_started
                ON backup_history (started_at);
            CREATE TABLE IF NOT EXISTS schedules (
                id            TEXT PRIMARY KEY,
                name          TEXT NOT NULL,
                plan_ids_json TEXT NOT NULL,
                cron_expr     TEXT NOT NULL,
                enabled       INTEGER NOT NULL DEFAULT 1,
                last_run_at   INTEGER,
                next_run_at   INTEGER,
                created_at    INTEGER NOT NULL
            );",
        )?;
        // Migrations for existing installs — silently ignored if columns already exist.
        let _ = conn.execute_batch("ALTER TABLE backup_plans ADD COLUMN limit_upload INTEGER;");
        let _ = conn.execute_batch("ALTER TABLE backup_plans ADD COLUMN limit_download INTEGER;");
        // Reset any mid-index state left by a crash or unexpected close.
        let _ = conn.execute_batch(
            "UPDATE browse_cache_status SET status = 'pending' WHERE status = 'in_progress';",
        );
        Ok(())
    }

    // ── master key ──────────────────────────────────────────────────────────

    pub fn has_master_key(&self) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row("SELECT 1 FROM master_key WHERE id = 1", [], |_| Ok(())) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn store_master_key(
        &self,
        salt: &[u8],
        verification_nonce: &[u8],
        verification_ciphertext: &[u8],
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO master_key
             (id, salt, verification_nonce, verification_ciphertext)
             VALUES (1, ?1, ?2, ?3)",
            params![salt, verification_nonce, verification_ciphertext],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_master_key_row(&self) -> Result<MasterKeyRow, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT salt, verification_nonce, verification_ciphertext FROM master_key WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .map_err(|e| e.to_string())
    }

    // ── repositories ────────────────────────────────────────────────────────

    pub fn list_repos(&self) -> Result<Vec<Repository>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, path FROM repositories ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let repos = stmt
            .query_map([], |row| {
                Ok(Repository {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(repos)
    }

    pub fn get_full_repo(&self, repo_id: &str, key: &[u8; 32]) -> Result<FullRepository, String> {
        let (path, nonce, ciphertext) = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT path, password_nonce, password_ciphertext
                 FROM repositories WHERE id = ?1",
                params![repo_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .map_err(|e| format!("Repository not found: {e}"))?
        };
        let password_bytes = crypto::decrypt(key, &nonce, &ciphertext)?;
        let password = String::from_utf8(password_bytes).map_err(|e| e.to_string())?;
        Ok(FullRepository { path, password })
    }

    pub fn add_repo(
        &self,
        id: &str,
        name: &str,
        path: &str,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO repositories (id, name, path, password_nonce, password_ciphertext)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, path, nonce, ciphertext],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_repo(&self, repo_id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM repositories WHERE id = ?1", params![repo_id])
            .map_err(|e| e.to_string())?;

        // Cascade cleanup: remove browse cache entries for this repo's snapshots.
        // browse_cache_status is keyed by (repo_id, snapshot_id).
        tx.execute(
            "DELETE FROM browse_cache_status WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        // browse_cache_files/indexed_snapshots are keyed by snapshot_id without
        // repo_id, so we delete all rows for snapshots that belong to this repo
        // (via snapshots_cache) before that table itself is cleared below.
        tx.execute(
            "DELETE FROM browse_cache_files WHERE snap IN (
                SELECT id FROM indexed_snapshots WHERE snapshot_id IN
                    (SELECT snapshot_id FROM snapshots_cache WHERE repo_id = ?1)
             )",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM indexed_snapshots WHERE snapshot_id IN
                (SELECT snapshot_id FROM snapshots_cache WHERE repo_id = ?1)",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM snapshots_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn rename_repo(&self, repo_id: &str, new_name: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE repositories SET name = ?1 WHERE id = ?2",
            params![new_name, repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn update_repo_path(&self, repo_id: &str, new_path: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE repositories SET path = ?1 WHERE id = ?2",
            params![new_path, repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn update_repo_password(
        &self,
        repo_id: &str,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE repositories SET password_nonce = ?1, password_ciphertext = ?2 WHERE id = ?3",
            params![nonce, ciphertext, repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Atomically rotate the master key: re-encrypt every repo password with the
    /// new key and rewrite the verification row in a single transaction. Either all
    /// of it commits or none of it does — so a crash can't leave repo passwords on
    /// the new key while the verification row still expects the old password (which
    /// would lock the user out and brick every repo).
    pub fn rotate_master_key(
        &self,
        old_key: &[u8; 32],
        new_key: &[u8; 32],
        new_salt: &[u8],
        new_verification_nonce: &[u8],
        new_verification_ciphertext: &[u8],
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        // Re-encrypt every repo password with the new key. If any row fails to
        // decrypt, the `?` returns and the transaction is rolled back on drop.
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> = {
            let mut stmt = tx
                .prepare("SELECT id, password_nonce, password_ciphertext FROM repositories")
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        for (id, nonce, ct) in rows {
            let mut pw = crypto::decrypt(old_key, &nonce, &ct)?;
            let (new_nonce, new_ct) = crypto::encrypt(new_key, &pw)?;
            pw.zeroize();
            tx.execute(
                "UPDATE repositories SET password_nonce = ?1, password_ciphertext = ?2 WHERE id = ?3",
                params![new_nonce, new_ct, id],
            )
            .map_err(|e| e.to_string())?;
        }

        // Rewrite the verification row in the same transaction.
        tx.execute(
            "INSERT OR REPLACE INTO master_key
             (id, salt, verification_nonce, verification_ciphertext)
             VALUES (1, ?1, ?2, ?3)",
            params![new_salt, new_verification_nonce, new_verification_ciphertext],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── backup plans ────────────────────────────────────────────────────────

    pub fn list_backup_plans(&self) -> Result<Vec<BackupPlan>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download FROM backup_plans ORDER BY name COLLATE NOCASE")
            .map_err(|e| e.to_string())?;
        let plans = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<u32>>(8)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        plans
            .into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download)| {
                Ok(BackupPlan {
                    id,
                    name,
                    repo_id,
                    paths: serde_json::from_str(&paths_json).map_err(|e| e.to_string())?,
                    tags: serde_json::from_str(&tags_json).map_err(|e| e.to_string())?,
                    excludes: serde_json::from_str(&excludes_json).map_err(|e| e.to_string())?,
                    retention: retention_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?,
                    limit_upload,
                    limit_download,
                })
            })
            .collect()
    }

    pub fn save_backup_plan(&self, plan: &BackupPlan) -> Result<(), String> {
        let paths_json = serde_json::to_string(&plan.paths).map_err(|e| e.to_string())?;
        let tags_json = serde_json::to_string(&plan.tags).map_err(|e| e.to_string())?;
        let excludes_json = serde_json::to_string(&plan.excludes).map_err(|e| e.to_string())?;
        let retention_json = plan
            .retention
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e: serde_json::Error| e.to_string())?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO backup_plans
             (id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                plan.id,
                plan.name,
                plan.repo_id,
                paths_json,
                tags_json,
                excludes_json,
                retention_json,
                plan.limit_upload,
                plan.limit_download,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_backup_plan(&self, plan_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM backup_plans WHERE id = ?1", params![plan_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── settings ────────────────────────────────────────────────────────────

    pub fn get_setting(&self, key: &str, default: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        ) {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(default.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── browse cache ─────────────────────────────────────────────────────────

    /// Looks up (or creates) the interned integer key for a snapshot's hex id
    /// in `indexed_snapshots`. Used by writers before inserting into
    /// `browse_cache_files`.
    fn intern_snapshot(
        tx: &rusqlite::Transaction,
        snapshot_id: &str,
    ) -> Result<i64, String> {
        tx.execute(
            "INSERT OR IGNORE INTO indexed_snapshots (snapshot_id) VALUES (?1)",
            params![snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        tx.query_row(
            "SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1",
            params![snapshot_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    /// Looks up the interned integer key for a snapshot's hex id, if it has
    /// ever been indexed. Used by readers/deleters — `None` means the
    /// snapshot has no rows in `browse_cache_files`.
    fn snap_id_of(conn: &Connection, snapshot_id: &str) -> Result<Option<i64>, String> {
        match conn.query_row(
            "SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1",
            params![snapshot_id],
            |row| row.get(0),
        ) {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn get(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        path: Option<&str>,
    ) -> Result<Option<Vec<FileEntry>>, String> {
        let parent_key = path.unwrap_or("");
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // Inlined former is_fully_indexed() — folded into this single locked scope so
        // the directory read below doesn't need a second lock acquisition.
        let fully_indexed = match conn.query_row(
            "SELECT 1 FROM browse_cache_status WHERE repo_id = ?1 AND snapshot_id = ?2 AND status = 'complete'",
            params![repo_id, snapshot_id],
            |_| Ok(()),
        ) {
            Ok(_) => true,
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => return Err(e.to_string()),
        };
        let snap = Self::snap_id_of(&conn, snapshot_id)?;
        let entries = match snap {
            None => Vec::new(),
            Some(snap) => {
                let mut stmt = conn
                    .prepare_cached(
                        "SELECT path, entry_type, size, mtime, mode
                         FROM browse_cache_files
                         WHERE snap = ?1 AND parent_path = ?2",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![snap, parent_key], |row| {
                        let path: String = row.get(0)?;
                        Ok(FileEntry {
                            name: name_of(&path),
                            path,
                            entry_type: row.get(1)?,
                            size: row.get(2)?,
                            mtime: row.get(3)?,
                            mode: row.get(4)?,
                        })
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                rows
            }
        };

        if fully_indexed {
            // Fully indexed: always return Some (empty vec = empty directory, not a cache miss)
            Ok(Some(entries))
        } else if !entries.is_empty() {
            // Partially indexed: return whatever was cached for this directory
            Ok(Some(entries))
        } else {
            Ok(None)
        }
    }

    pub fn set(
        &self,
        snapshot_id: &str,
        path: Option<&str>,
        entries: &[FileEntry],
    ) -> Result<(), String> {
        let parent_key = path.unwrap_or("");
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let snap = Self::intern_snapshot(&tx, snapshot_id)?;
        tx.execute(
            "DELETE FROM browse_cache_files WHERE snap = ?1 AND parent_path = ?2",
            params![snap, parent_key],
        )
        .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO browse_cache_files
                     (snap, path, parent_path, entry_type, size, mtime, mode)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )
                .map_err(|e| e.to_string())?;
            for entry in entries {
                stmt.execute(params![
                    snap,
                    entry.path,
                    parent_key,
                    entry.entry_type,
                    entry.size,
                    entry.mtime,
                    entry.mode,
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn evict(&self, repo_id: &str, snapshot_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM browse_cache_files WHERE snap =
             (SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1)",
            params![snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM indexed_snapshots WHERE snapshot_id = ?1",
            params![snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM browse_cache_status WHERE repo_id = ?1 AND snapshot_id = ?2",
            params![repo_id, snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── browse cache status ───────────────────────────────────────────────────

    pub fn get_browse_status(
        &self,
        repo_id: &str,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT snapshot_id, status FROM browse_cache_status WHERE repo_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let map = stmt
            .query_map(params![repo_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(map)
    }

    pub fn set_browse_status(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        status: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO browse_cache_status (repo_id, snapshot_id, status, cached_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![repo_id, snapshot_id, status, timestamp()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Full-text substring search across all indexed files in a snapshot.
    /// Matches against the full path (which subsumes matching by name).
    pub fn search_browse_files(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<FileEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let snap = Self::snap_id_of(&conn, snapshot_id)?;
        let Some(snap) = snap else {
            return Ok(Vec::new());
        };
        // Escape LIKE metacharacters in the user's query so they're treated literally.
        let pattern = format!("%{}%", query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_"));
        let mut stmt = conn
            .prepare_cached(
                "SELECT path, entry_type, size, mtime, mode
                 FROM browse_cache_files
                 WHERE snap = ?1
                   AND path LIKE ?2 ESCAPE '\\'
                 ORDER BY path
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let entries = stmt
            .query_map(params![snap, pattern, limit as i64], |row| {
                let path: String = row.get(0)?;
                Ok(FileEntry {
                    name: name_of(&path),
                    path,
                    entry_type: row.get(1)?,
                    size: row.get(2)?,
                    mtime: row.get(3)?,
                    mode: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(entries)
    }

    /// Searches all fully-indexed snapshots of a repo. Each matching path is
    /// returned once, resolved to the newest snapshot containing it — `GROUP BY path`
    /// collapses duplicates and the `MAX(sc.time)` + join-back picks the winning row's
    /// snapshot_id/short_id via SQLite's "bare column takes the row of the MAX aggregate"
    /// behavior within a GROUP BY (each column comes from the same row as the MAX).
    pub fn search_repo_files(
        &self,
        repo_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<super::browse::RepoFileHit>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let pattern = format!("%{}%", query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_"));
        let mut stmt = conn
            .prepare_cached(
                "SELECT bcf.path, bcf.entry_type, bcf.size, bcf.mtime, bcf.mode,
                        isn.snapshot_id, sc.short_id, MAX(sc.time)
                 FROM browse_cache_files bcf
                 JOIN indexed_snapshots isn
                   ON isn.id = bcf.snap
                 JOIN snapshots_cache sc
                   ON sc.snapshot_id = isn.snapshot_id AND sc.repo_id = ?1
                 JOIN browse_cache_status bcs
                   ON bcs.snapshot_id = isn.snapshot_id AND bcs.repo_id = ?1 AND bcs.status = 'complete'
                 WHERE bcf.path LIKE ?2 ESCAPE '\\'
                 GROUP BY bcf.path
                 ORDER BY bcf.path
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let hits = stmt
            .query_map(params![repo_id, pattern, limit as i64], |row| {
                let path: String = row.get(0)?;
                Ok(super::browse::RepoFileHit {
                    name: name_of(&path),
                    path,
                    entry_type: row.get(1)?,
                    size: row.get(2)?,
                    mtime: row.get(3)?,
                    mode: row.get(4)?,
                    snapshot_id: row.get(5)?,
                    snapshot_short_id: row.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(hits)
    }

    /// Bulk-insert file entries for a snapshot (used by the cache warmer and manual indexing).
    /// Inserts in chunks of 500 to avoid holding the mutex for excessive time.
    pub fn insert_browse_files(
        &self,
        snapshot_id: &str,
        entries: &[FileEntry],
    ) -> Result<(), String> {
        // Resolve the interned snapshot id once up front — snap is constant across
        // every chunk, so re-interning inside the loop (as before) was a redundant
        // INSERT OR IGNORE + SELECT per chunk on the bulk-index hot path.
        let snap = {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            let snap = Self::intern_snapshot(&tx, snapshot_id)?;
            tx.commit().map_err(|e| e.to_string())?;
            snap
        };
        for chunk in entries.chunks(500) {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            {
                let mut stmt = tx
                    .prepare_cached(
                        "INSERT OR REPLACE INTO browse_cache_files
                         (snap, path, parent_path, entry_type, size, mtime, mode)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(|e| e.to_string())?;
                for entry in chunk {
                    let parent = parent_path_of(&entry.path);
                    stmt.execute(params![
                        snap,
                        entry.path,
                        parent,
                        entry.entry_type,
                        entry.size,
                        entry.mtime,
                        entry.mode,
                    ])
                    .map_err(|e| e.to_string())?;
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Returns the next (repo_id, snapshot_id) that needs indexing from eligible repos,
    /// preferring snapshots with no status entry, then those with status = 'pending'.
    pub fn get_next_unindexed_snapshot(
        &self,
        eligible_repo_ids: &[String],
    ) -> Result<Option<(String, String)>, String> {
        if eligible_repo_ids.is_empty() {
            return Ok(None);
        }
        let placeholders = eligible_repo_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT sc.repo_id, sc.snapshot_id
             FROM snapshots_cache sc
             LEFT JOIN browse_cache_status bcs
                 ON bcs.repo_id = sc.repo_id AND bcs.snapshot_id = sc.snapshot_id
             WHERE sc.repo_id IN ({placeholders})
               AND (bcs.status IS NULL OR bcs.status = 'pending')
             LIMIT 1"
        );
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        match stmt.query_row(rusqlite::params_from_iter(eligible_repo_ids.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Aggregate indexing progress across the given eligible repos: how many of their
    /// cached snapshots have a `browse_cache_status` row of `complete` vs. the total
    /// snapshot count. Backs the Activity panel's single "N of M indexed" figure so the
    /// frontend doesn't have to fetch snapshot lists + per-repo index status and sum them
    /// itself. Mirrors the eligibility filtering `get_next_unindexed_snapshot` uses.
    pub fn get_index_progress(&self, eligible_repo_ids: &[String]) -> Result<(u64, u64), String> {
        if eligible_repo_ids.is_empty() {
            return Ok((0, 0));
        }
        let placeholders = eligible_repo_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(", ");
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let total_sql = format!(
            "SELECT COUNT(*) FROM snapshots_cache WHERE repo_id IN ({placeholders})"
        );
        let total: u64 = conn
            .query_row(&total_sql, rusqlite::params_from_iter(eligible_repo_ids.iter()), |row| row.get(0))
            .map_err(|e| e.to_string())?;

        let cached_sql = format!(
            "SELECT COUNT(*) FROM browse_cache_status
             WHERE repo_id IN ({placeholders}) AND status = 'complete'"
        );
        let cached: u64 = conn
            .query_row(&cached_sql, rusqlite::params_from_iter(eligible_repo_ids.iter()), |row| row.get(0))
            .map_err(|e| e.to_string())?;

        Ok((cached, total))
    }

    // ── snapshots cache ──────────────────────────────────────────────────────

    /// Whether `snapshots_cache` currently holds any row for this repo. Used by
    /// the cache warmer to detect a cache that was wiped out-of-band (e.g. the
    /// Settings page's "Clear All Cache"/"Clean Orphaned Data" buttons), which its
    /// in-memory last-seen-hash map has no way to observe on its own.
    pub fn has_cached_snapshots(&self, repo_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM snapshots_cache WHERE repo_id = ?1",
                params![repo_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        Ok(count > 0)
    }

    /// Returns cached snapshots for a repo as structs directly — no JSON string
    /// round-trip (the caller previously re-parsed a serialized string this method
    /// produced; see `list_snapshots` in `snapshot.rs`).
    pub fn get_snapshots_vec(&self, repo_id: &str) -> Result<Vec<Snapshot>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT snapshot_id, short_id, time, hostname, username, paths, tags
                 FROM snapshots_cache WHERE repo_id = ?1
                 ORDER BY time ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![repo_id], |row| {
                let paths: String = row.get(5)?;
                let tags: Option<String> = row.get(6)?;
                Ok(Snapshot {
                    id: row.get(0)?,
                    short_id: row.get(1)?,
                    time: row.get(2)?,
                    hostname: row.get(3)?,
                    username: row.get(4)?,
                    paths: serde_json::from_str(&paths).unwrap_or_default(),
                    tags: tags.and_then(|t| serde_json::from_str(&t).ok()),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Full replace: clears existing snapshot rows for this repo, then inserts all from JSON.
    pub fn set_snapshots(&self, repo_id: &str, json: &str) -> Result<(), String> {
        let rows = parse_snapshot_rows(json)?;
        let now = timestamp();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM snapshots_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO snapshots_cache
                     (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| e.to_string())?;
            for s in &rows {
                stmt.execute(params![
                    repo_id,
                    s.id,
                    s.short_id,
                    s.time,
                    s.hostname,
                    s.username,
                    s.paths,
                    s.tags,
                    now
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Upsert-only: inserts new snapshot rows without clearing existing ones.
    /// Used by execute_backup to add a newly created snapshot to the cache.
    pub fn append_snapshots(&self, repo_id: &str, json: &str) -> Result<(), String> {
        let rows = parse_snapshot_rows(json)?;
        let now = timestamp();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        // Wrapped in a transaction so N appended rows (e.g. a batch backup run) commit
        // as a single fsync instead of one implicit autocommit transaction per row.
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO snapshots_cache
                     (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|e| e.to_string())?;
            for s in &rows {
                stmt.execute(params![
                    repo_id,
                    s.id,
                    s.short_id,
                    s.time,
                    s.hostname,
                    s.username,
                    s.paths,
                    s.tags,
                    now
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn evict_snapshots(&self, repo_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM snapshots_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── repo stats cache ─────────────────────────────────────────────────────

    /// Returns `(total_size, total_file_count, snapshots_count, cached_at)`. `cached_at`
    /// is a Unix-seconds timestamp — surfaced to the frontend as a "Refreshed …" label
    /// on RepositoriesPage now that stats are manual-refresh-only (see `set_stats`).
    pub fn get_stats(&self, repo_id: &str) -> Result<Option<(u64, u64, u64, i64)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT total_size, total_file_count, snapshots_count, cached_at
             FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, i64>(3)?,
                ))
            },
        ) {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Writes fresh stats and returns the `cached_at` timestamp it wrote, so the caller
    /// (`fetch_and_cache_stats` in repo.rs) can hand it straight back to the frontend
    /// without a re-read.
    pub fn set_stats(
        &self,
        repo_id: &str,
        total_size: u64,
        total_file_count: u64,
        snapshots_count: u64,
    ) -> Result<i64, String> {
        let now = timestamp();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO repo_stats_cache
             (repo_id, total_size, total_file_count, snapshots_count, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                repo_id,
                total_size as i64,
                total_file_count as i64,
                snapshots_count as i64,
                now
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(now)
    }

    // ── backup history ────────────────────────────────────────────────────────

    pub fn list_backup_history(&self) -> Result<Vec<BackupHistoryEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT h.id, h.repo_id, r.name, h.plan_id, p.name,
                        h.snapshot_id, h.started_at, h.duration_seconds,
                        h.files_new, h.files_changed, h.bytes_added, h.error
                 FROM backup_history h
                 LEFT JOIN repositories r ON r.id = h.repo_id
                 LEFT JOIN backup_plans p ON p.id = h.plan_id
                 ORDER BY h.started_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![BACKUP_HISTORY_LIMIT], |row| {
                Ok(BackupHistoryEntry {
                    id: row.get(0)?,
                    repo_id: row.get(1)?,
                    repo_name: row.get(2)?,
                    plan_id: row.get(3)?,
                    plan_name: row.get(4)?,
                    snapshot_id: row.get(5)?,
                    started_at: row.get(6)?,
                    duration_seconds: row.get(7)?,
                    files_new: row.get::<_, i64>(8)? as u64,
                    files_changed: row.get::<_, i64>(9)? as u64,
                    bytes_added: row.get::<_, i64>(10)? as u64,
                    error: row.get(11)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    // ── backup history (insert) ───────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn log_backup(
        &self,
        id: &str,
        repo_id: &str,
        plan_id: Option<&str>,
        snapshot_id: Option<&str>,
        started_at: i64,
        duration_seconds: f64,
        files_new: u64,
        files_changed: u64,
        bytes_added: u64,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO backup_history
             (id, repo_id, plan_id, snapshot_id, started_at, duration_seconds,
              files_new, files_changed, bytes_added, error)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id, repo_id, plan_id, snapshot_id, started_at, duration_seconds,
                files_new as i64, files_changed as i64, bytes_added as i64, error
            ],
        )
        .map_err(|e| e.to_string())?;
        // Trim to the newest BACKUP_HISTORY_LIMIT rows so the table can't grow
        // without bound. Runs after the insert is already persisted. Guarded by a
        // count check so a normal backup (table under the cap) skips the DELETE.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM backup_history", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if count > BACKUP_HISTORY_LIMIT {
            conn.execute(
                "DELETE FROM backup_history WHERE id NOT IN (
                     SELECT id FROM backup_history ORDER BY started_at DESC LIMIT ?1
                 )",
                params![BACKUP_HISTORY_LIMIT],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ── schedules ────────────────────────────────────────────────────────────

    pub fn list_schedules(&self) -> Result<Vec<Schedule>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at
                 FROM schedules ORDER BY created_at",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|(id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at)| {
                Ok(Schedule {
                    id,
                    name,
                    plan_ids: serde_json::from_str(&plan_ids_json).map_err(|e: serde_json::Error| e.to_string())?,
                    cron_expr,
                    enabled: enabled != 0,
                    last_run_at,
                    next_run_at,
                    created_at,
                })
            })
            .collect()
    }

    pub fn save_schedule(&self, s: &Schedule) -> Result<(), String> {
        let plan_ids_json = serde_json::to_string(&s.plan_ids).map_err(|e| e.to_string())?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO schedules
             (id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                s.id,
                s.name,
                plan_ids_json,
                s.cron_expr,
                s.enabled as i64,
                s.last_run_at,
                s.next_run_at,
                s.created_at
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn remove_schedule(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM schedules WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_schedule_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE schedules SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn list_due_schedules(&self, now: i64) -> Result<Vec<Schedule>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at
                 FROM schedules WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![now], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|(id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at)| {
                Ok(Schedule {
                    id,
                    name,
                    plan_ids: serde_json::from_str(&plan_ids_json).map_err(|e: serde_json::Error| e.to_string())?,
                    cron_expr,
                    enabled: enabled != 0,
                    last_run_at,
                    next_run_at,
                    created_at,
                })
            })
            .collect()
    }

    /// On startup, advance any overdue `next_run_at` values to the next future fire time.
    /// This skips missed backups (app was closed) rather than running them all at once.
    pub fn recalculate_overdue_schedules(&self, now: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, cron_expr FROM schedules WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at < ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![now], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        for (id, cron_expr) in rows {
            let full = format!("0 {} *", cron_expr.trim());
            if let Ok(sched) = CronSchedule::from_str(&full) {
                if let Some(next) = sched.upcoming(Local).next() {
                    let _ = conn.execute(
                        "UPDATE schedules SET next_run_at = ?1 WHERE id = ?2",
                        params![next.timestamp(), id],
                    );
                }
            }
        }
        Ok(())
    }

    pub fn record_schedule_run(&self, id: &str, ran_at: i64, next_run_at: Option<i64>) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE schedules SET last_run_at = ?1, next_run_at = ?2 WHERE id = ?3",
            params![ran_at, next_run_at, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_plans_for_ids(&self, ids: &[String]) -> Result<Vec<BackupPlan>, String> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let placeholders = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download
             FROM backup_plans WHERE id IN ({})",
            placeholders
        );
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(ids.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<u32>>(7)?,
                    row.get::<_, Option<u32>>(8)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download)| {
                Ok(BackupPlan {
                    id,
                    name,
                    repo_id,
                    paths: serde_json::from_str(&paths_json).map_err(|e: serde_json::Error| e.to_string())?,
                    tags: serde_json::from_str(&tags_json).map_err(|e: serde_json::Error| e.to_string())?,
                    excludes: serde_json::from_str(&excludes_json).map_err(|e: serde_json::Error| e.to_string())?,
                    retention: retention_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?,
                    limit_upload,
                    limit_download,
                })
            })
            .collect()
    }

    // ── size helper ──────────────────────────────────────────────────────────

    /// Checkpoint the WAL into the main file, then return the combined on-disk
    /// size of `app_data.db` + `app_data.db-wal`. Must be called while the
    /// `Connection` mutex is already held by the caller so no background thread
    /// can append WAL frames between the checkpoint and the `fs::metadata` reads.
    fn checkpoint_and_size(&self, conn: &Connection) -> u64 {
        // TRUNCATE mode moves all checkpointed frames into the main file and
        // zeros the WAL, so both files reflect the true post-operation footprint.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        let main = std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
        let wal = std::fs::metadata(self.db_path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        main + wal
    }

    /// Public entry-point for the `get_db_size` command: acquires the
    /// connection lock, checkpoints the WAL, and returns the combined size.
    pub fn get_size(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Ok(self.checkpoint_and_size(&conn))
    }

    // ── global clear ─────────────────────────────────────────────────────────

    pub fn clear_cache(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "DELETE FROM browse_cache_files;
             DELETE FROM indexed_snapshots;
             DELETE FROM browse_cache_status;
             DELETE FROM snapshots_cache;
             DELETE FROM repo_stats_cache;",
        )
        .map_err(|e| e.to_string())?;
        // VACUUM rewrites all live pages into the WAL (in WAL mode). Checkpoint
        // afterwards moves those compacted pages into the main file and truncates
        // the WAL, so both files end up small.
        conn.execute_batch("VACUUM;").map_err(|e| e.to_string())?;
        Ok(self.checkpoint_and_size(&conn))
    }

    /// Remove only orphaned cache rows, leaving live caches intact. Returns
    /// `(rows_deleted, db_size_bytes)`. Orphans are:
    ///   - `snapshots_cache` / `repo_stats_cache` rows whose `repo_id` no longer
    ///     exists in `repositories` (e.g. a deleted repo),
    ///   - `browse_cache_files` / `browse_cache_status` / `indexed_snapshots`
    ///     rows whose `snapshot_id` is not referenced by any remaining
    ///     `snapshots_cache` entry.
    pub fn clean_cache(&self) -> Result<(u64, u64), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut removed = 0u64;

        removed += tx
            .execute(
                "DELETE FROM snapshots_cache
                 WHERE repo_id NOT IN (SELECT id FROM repositories)",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        removed += tx
            .execute(
                "DELETE FROM repo_stats_cache
                 WHERE repo_id NOT IN (SELECT id FROM repositories)",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        removed += tx
            .execute(
                "DELETE FROM browse_cache_files
                 WHERE snap IN (
                     SELECT id FROM indexed_snapshots
                     WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)
                 )",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        removed += tx
            .execute(
                "DELETE FROM indexed_snapshots
                 WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        removed += tx
            .execute(
                "DELETE FROM browse_cache_status
                 WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        tx.commit().map_err(|e| e.to_string())?;
        // Checkpoint under the still-held lock so the size read sees exactly
        // what's on disk, with no background WAL writes racing in between.
        let size = self.checkpoint_and_size(&conn);
        Ok((removed, size))
    }

    /// Rewrite the database file to reclaim free pages, without deleting any
    /// rows. Unlike `clear_cache`, this never touches live data — it's a plain
    /// `VACUUM` for users who just want to recover disk space.
    pub fn compress_database(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("VACUUM;").map_err(|e| e.to_string())?;
        Ok(self.checkpoint_and_size(&conn))
    }

    /// Wipe all user data. Returns app to first-launch state.
    pub fn reset_all(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "BEGIN;
             DELETE FROM master_key;
             DELETE FROM repositories;
             DELETE FROM backup_plans;
             DELETE FROM app_settings;
             DELETE FROM browse_cache_files;
             DELETE FROM indexed_snapshots;
             DELETE FROM browse_cache_status;
             DELETE FROM snapshots_cache;
             DELETE FROM repo_stats_cache;
             DELETE FROM backup_history;
             DELETE FROM schedules;
             COMMIT;",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Insert imported repositories, backup plans, and schedules in a single
    /// transaction. Repo passwords are passed already re-encrypted under the
    /// local master key (nonce + ciphertext). IDs are pre-generated and all
    /// cross-references already remapped by the caller. All-or-nothing — any
    /// failure rolls the entire import back, so a partial import can't leave
    /// dangling references.
    pub fn import_bundle(
        &self,
        repos: &[ImportRepo],
        plans: &[BackupPlan],
        schedules: &[Schedule],
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        for r in repos {
            tx.execute(
                "INSERT INTO repositories (id, name, path, password_nonce, password_ciphertext)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![r.id, r.name, r.path, r.password_nonce, r.password_ciphertext],
            )
            .map_err(|e| e.to_string())?;
        }

        for plan in plans {
            let paths_json = serde_json::to_string(&plan.paths).map_err(|e| e.to_string())?;
            let tags_json = serde_json::to_string(&plan.tags).map_err(|e| e.to_string())?;
            let excludes_json = serde_json::to_string(&plan.excludes).map_err(|e| e.to_string())?;
            let retention_json = plan
                .retention
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e: serde_json::Error| e.to_string())?;
            tx.execute(
                "INSERT INTO backup_plans
                 (id, name, repo_id, paths_json, tags_json, excludes_json, retention_json, limit_upload, limit_download)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    plan.id, plan.name, plan.repo_id, paths_json, tags_json, excludes_json,
                    retention_json, plan.limit_upload, plan.limit_download,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        for s in schedules {
            let plan_ids_json = serde_json::to_string(&s.plan_ids).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO schedules
                 (id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    s.id, s.name, plan_ids_json, s.cron_expr, s.enabled as i64,
                    s.last_run_at, s.next_run_at, s.created_at,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// A repository row prepared for import: password already re-encrypted under the
/// local master key.
pub struct ImportRepo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub password_nonce: Vec<u8>,
    pub password_ciphertext: Vec<u8>,
}

fn timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compute the parent directory path for a file path from `restic ls` output.
/// `/foo/bar/baz.txt` → `/foo/bar`, `/foo` → `""`, `` → `""`.
pub(crate) fn parent_path_of(path: &str) -> String {
    let clean = path.trim_end_matches('/');
    match clean.rfind('/') {
        None | Some(0) => String::new(),
        Some(i) => clean[..i].to_string(),
    }
}

/// Compute the file/dir name (last path segment) from a `restic ls` path.
/// `/foo/bar/baz.txt` → `baz.txt`, `/foo` → `foo`. Used to rebuild the `name`
/// field on read now that it's no longer stored in `browse_cache_files`.
pub(crate) fn name_of(path: &str) -> String {
    let clean = path.trim_end_matches('/');
    match clean.rfind('/') {
        None => clean.to_string(),
        Some(i) => clean[i + 1..].to_string(),
    }
}

struct SnapshotRow {
    id: String,
    short_id: String,
    time: String,
    hostname: String,
    username: Option<String>,
    paths: String,
    tags: Option<String>,
}

fn parse_snapshot_rows(json: &str) -> Result<Vec<SnapshotRow>, String> {
    #[derive(Deserialize)]
    struct Raw {
        id: String,
        short_id: String,
        time: String,
        hostname: String,
        username: Option<String>,
        paths: Vec<String>,
        tags: Option<Vec<String>>,
    }
    let raws: Vec<Raw> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    raws.into_iter()
        .map(|r| {
            Ok(SnapshotRow {
                id: r.id,
                short_id: r.short_id,
                time: r.time,
                hostname: r.hostname,
                username: r.username,
                paths: serde_json::to_string(&r.paths).map_err(|e| e.to_string())?,
                tags: r
                    .tags
                    .map(|t| serde_json::to_string(&t))
                    .transpose()
                    .map_err(|e: serde_json::Error| e.to_string())?,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn clear_browse_cache(app: tauri::AppHandle) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        db.clear_cache()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn clean_cache(db: tauri::State<'_, AppDb>) -> Result<(u64, u64), String> {
    db.clean_cache()
}

#[tauri::command]
pub fn get_db_size(db: tauri::State<'_, AppDb>) -> Result<u64, String> {
    db.get_size()
}

#[tauri::command]
pub async fn compress_database(app: tauri::AppHandle) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<AppDb>();
        db.compress_database()
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_backup_history(db: tauri::State<'_, AppDb>) -> Result<Vec<BackupHistoryEntry>, String> {
    db.list_backup_history()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> AppDb {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        AppDb::init_schema(&conn).unwrap();
        AppDb::new(conn, std::path::PathBuf::new())
    }

    #[test]
    fn test_evict_preserves_other_repo_status() {
        let db = test_db();

        // Insert two repos' browse_cache_status for the same snapshot_id.
        db.set_browse_status("repoA", "snap123", "complete").unwrap();
        db.set_browse_status("repoB", "snap123", "complete").unwrap();

        // Evict from repoA only.
        db.evict("repoA", "snap123").unwrap();

        // Verify repoA's status is gone.
        let status_a = db.get_browse_status("repoA").unwrap();
        assert!(!status_a.contains_key("snap123"));

        // Verify repoB's status remains.
        let status_b = db.get_browse_status("repoB").unwrap();
        assert_eq!(status_b.get("snap123"), Some(&"complete".to_string()));
    }

    #[test]
    fn set_stats_returns_and_persists_cached_at() {
        let db = test_db();

        let ts1 = db.set_stats("repoA", 100, 5, 2).unwrap();
        let (total_size, total_file_count, snapshots_count, cached_at) =
            db.get_stats("repoA").unwrap().unwrap();
        assert_eq!((total_size, total_file_count, snapshots_count), (100, 5, 2));
        assert_eq!(cached_at, ts1);

        // A later set_stats overwrites the value and advances cached_at (or at least
        // never goes backwards — timestamp() is second-resolution, so two calls in the
        // same test can legitimately land on the same second).
        let ts2 = db.set_stats("repoA", 200, 8, 3).unwrap();
        assert!(ts2 >= ts1);
        let (total_size, total_file_count, snapshots_count, cached_at) =
            db.get_stats("repoA").unwrap().unwrap();
        assert_eq!((total_size, total_file_count, snapshots_count), (200, 8, 3));
        assert_eq!(cached_at, ts2);
    }

    #[test]
    fn search_repo_files_dedups_to_newest_snapshot_and_excludes_unindexed() {
        let db = test_db();
        let repo_id = "repoA";

        // Two indexed ("complete") snapshots, both containing the same path,
        // plus one pending snapshot whose files must be excluded entirely.
        let json = r#"[
            {"id":"snap-old00000000","short_id":"snapold0","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"]},
            {"id":"snap-new00000000","short_id":"snapnew0","time":"2024-06-01T00:00:00Z","hostname":"host","paths":["/home"]},
            {"id":"snap-pending0000","short_id":"snappend","time":"2024-09-01T00:00:00Z","hostname":"host","paths":["/home"]}
        ]"#;
        db.set_snapshots(repo_id, json).unwrap();
        db.set_browse_status(repo_id, "snap-old00000000", "complete").unwrap();
        db.set_browse_status(repo_id, "snap-new00000000", "complete").unwrap();
        db.set_browse_status(repo_id, "snap-pending0000", "pending").unwrap();

        let shared_entry = FileEntry {
            name: "notes.txt".to_string(),
            path: "/home/notes.txt".to_string(),
            entry_type: "file".to_string(),
            size: Some(10),
            mtime: None,
            mode: None,
        };
        let only_in_pending = FileEntry {
            name: "secret.txt".to_string(),
            path: "/home/secret.txt".to_string(),
            entry_type: "file".to_string(),
            size: Some(5),
            mtime: None,
            mode: None,
        };
        db.insert_browse_files("snap-old00000000", std::slice::from_ref(&shared_entry)).unwrap();
        db.insert_browse_files("snap-new00000000", &[shared_entry]).unwrap();
        db.insert_browse_files("snap-pending0000", &[only_in_pending]).unwrap();

        let hits = db.search_repo_files(repo_id, "notes", 200).unwrap();
        assert_eq!(hits.len(), 1, "duplicate path across snapshots should be deduped");
        assert_eq!(hits[0].path, "/home/notes.txt");
        assert_eq!(hits[0].snapshot_id, "snap-new00000000", "should resolve to the newest snapshot");
        assert_eq!(hits[0].snapshot_short_id, "snapnew0");

        let pending_hits = db.search_repo_files(repo_id, "secret", 200).unwrap();
        assert!(pending_hits.is_empty(), "files from a non-complete snapshot must be excluded");
    }

    fn seed_snapshot(db: &AppDb, repo_id: &str, snapshot_id: &str) {
        let json = format!(
            r#"[{{"id":"{snapshot_id}","short_id":"{snapshot_id}","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"]}}]"#
        );
        db.set_snapshots(repo_id, &json).unwrap();
    }

    #[test]
    fn get_next_unindexed_returns_none_for_empty_repo_list() {
        let db = test_db();
        assert!(db.get_next_unindexed_snapshot(&[]).unwrap().is_none());
    }

    #[test]
    fn get_next_unindexed_returns_snapshot_with_no_status_entry() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        // No browse_cache_status row — should be returned as unindexed.
        let result = db.get_next_unindexed_snapshot(&["repoA".to_string()]).unwrap();
        assert_eq!(result, Some(("repoA".to_string(), "aaaa111100000000".to_string())));
    }

    #[test]
    fn get_next_unindexed_returns_none_when_all_complete() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        db.set_browse_status("repoA", "aaaa111100000000", "complete").unwrap();
        assert!(db.get_next_unindexed_snapshot(&["repoA".to_string()]).unwrap().is_none());
    }

    #[test]
    fn get_index_progress_returns_zero_for_empty_repo_list() {
        let db = test_db();
        assert_eq!(db.get_index_progress(&[]).unwrap(), (0, 0));
    }

    #[test]
    fn get_index_progress_counts_complete_vs_total_across_eligible_repos() {
        let db = test_db();
        // set_snapshots is a full replace per repo_id, so both of repoA's snapshots must be
        // seeded in a single JSON array rather than via two seed_snapshot calls.
        let repo_a_json = r#"[
            {"id":"aaaa111100000000","short_id":"aaaa1111","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"]},
            {"id":"aaaa222200000000","short_id":"aaaa2222","time":"2024-02-01T00:00:00Z","hostname":"host","paths":["/home"]}
        ]"#;
        db.set_snapshots("repoA", repo_a_json).unwrap();
        seed_snapshot(&db, "repoB", "bbbb111100000000");
        db.set_browse_status("repoA", "aaaa111100000000", "complete").unwrap();
        db.set_browse_status("repoA", "aaaa222200000000", "pending").unwrap();
        // repoB's snapshot has no status row at all — still counts toward total, not cached.

        let (cached, total) = db
            .get_index_progress(&["repoA".to_string(), "repoB".to_string()])
            .unwrap();
        assert_eq!(cached, 1);
        assert_eq!(total, 3);
    }

    #[test]
    fn get_index_progress_ignores_repos_not_in_eligible_list() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        seed_snapshot(&db, "repoB", "bbbb111100000000");
        db.set_browse_status("repoB", "bbbb111100000000", "complete").unwrap();

        let (cached, total) = db.get_index_progress(&["repoA".to_string()]).unwrap();
        assert_eq!((cached, total), (0, 1), "repoB must not contribute when excluded");
    }

    #[test]
    fn get_next_unindexed_returns_pending_snapshot() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        db.set_browse_status("repoA", "aaaa111100000000", "pending").unwrap();
        let result = db.get_next_unindexed_snapshot(&["repoA".to_string()]).unwrap();
        assert_eq!(result, Some(("repoA".to_string(), "aaaa111100000000".to_string())));
    }

    #[test]
    fn get_next_unindexed_skips_complete_returns_unindexed_from_other_repo() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        seed_snapshot(&db, "repoB", "bbbb222200000000");
        db.set_browse_status("repoA", "aaaa111100000000", "complete").unwrap();
        // repoB has no status row — should be picked.
        let result = db
            .get_next_unindexed_snapshot(&["repoA".to_string(), "repoB".to_string()])
            .unwrap();
        assert_eq!(result, Some(("repoB".to_string(), "bbbb222200000000".to_string())));
    }

    #[test]
    fn get_next_unindexed_ignores_repos_not_in_eligible_list() {
        let db = test_db();
        seed_snapshot(&db, "repoA", "aaaa111100000000");
        // repoA has snapshots but is not in the eligible list.
        assert!(db.get_next_unindexed_snapshot(&["repoB".to_string()]).unwrap().is_none());
    }

    #[test]
    fn test_parent_path_of() {
        assert_eq!(parent_path_of("foo"), "");
        assert_eq!(parent_path_of("foo/"), "");
        assert_eq!(parent_path_of("foo/bar"), "foo");
        assert_eq!(parent_path_of("foo/bar/"), "foo");
        assert_eq!(parent_path_of("foo/bar/baz"), "foo/bar");
        assert_eq!(parent_path_of("foo/bar/baz/"), "foo/bar");
        assert_eq!(parent_path_of("a/b/c/d/e"), "a/b/c/d");
        assert_eq!(parent_path_of("/foo"), "");
        assert_eq!(parent_path_of("/"), "");
        assert_eq!(parent_path_of(""), "");
    }

    #[test]
    fn test_name_of() {
        assert_eq!(name_of("/foo/bar/baz.txt"), "baz.txt");
        assert_eq!(name_of("/foo"), "foo");
        assert_eq!(name_of("foo"), "foo");
        assert_eq!(name_of("/foo/bar/"), "bar");
        assert_eq!(name_of("foo/bar/"), "bar");
        assert_eq!(name_of("/"), "");
        assert_eq!(name_of(""), "");
    }

    #[test]
    fn test_parse_snapshot_rows() {
        let json = r#"[
            {"id": "abc123", "short_id": "abc123", "time": "2024-01-15T12:00:00Z",
             "hostname": "host1", "username": "user1", "paths": ["/foo", "/bar"], "tags": ["a", "b"]},
            {"id": "def456", "short_id": "def456", "time": "2024-01-16T12:00:00Z",
             "hostname": "host2", "username": null, "paths": ["/baz"], "tags": null}
        ]"#;
        let rows = parse_snapshot_rows(json).unwrap();
        assert_eq!(rows.len(), 2);

        assert_eq!(rows[0].id, "abc123");
        assert_eq!(rows[0].paths, serde_json::to_string(&vec!["/foo", "/bar"]).unwrap());
        assert_eq!(rows[0].tags, Some(serde_json::to_string(&vec!["a", "b"]).unwrap()));

        assert_eq!(rows[1].id, "def456");
        assert_eq!(rows[1].username, None);
        assert_eq!(rows[1].tags, None);
    }

    #[test]
    fn test_parse_snapshot_rows_invalid_json() {
        assert!(parse_snapshot_rows("not json").is_err());
        assert!(parse_snapshot_rows("{}").is_err());
    }

    // ── rotate_master_key ───────────────────────────────────────────────────

    fn make_key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn add_repo_encrypted(db: &AppDb, id: &str, name: &str, path: &str, password: &str, key: &[u8; 32]) {
        let (nonce, ct) = super::crypto::encrypt(key, password.as_bytes()).unwrap();
        db.add_repo(id, name, path, &nonce, &ct).unwrap();
    }

    #[test]
    fn rotate_master_key_reencrypts_all_repos() {
        let db = test_db();
        let old_key = make_key(1);
        let new_key = make_key(2);

        add_repo_encrypted(&db, "r1", "Repo One", "/path/one", "pw-one", &old_key);
        add_repo_encrypted(&db, "r2", "Repo Two", "/path/two", "pw-two", &old_key);

        let salt = [0u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        db.rotate_master_key(&old_key, &new_key, &salt, &vn, &vct).unwrap();

        let r1 = db.get_full_repo("r1", &new_key).unwrap();
        assert_eq!(r1.password, "pw-one");
        let r2 = db.get_full_repo("r2", &new_key).unwrap();
        assert_eq!(r2.password, "pw-two");
    }

    #[test]
    fn rotate_master_key_old_key_no_longer_works_after_rotation() {
        let db = test_db();
        let old_key = make_key(1);
        let new_key = make_key(2);

        add_repo_encrypted(&db, "r1", "Repo", "/path", "secret", &old_key);

        let salt = [0u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        db.rotate_master_key(&old_key, &new_key, &salt, &vn, &vct).unwrap();

        assert!(db.get_full_repo("r1", &old_key).is_err());
    }

    #[test]
    fn rotate_master_key_rolls_back_on_wrong_old_key() {
        let db = test_db();
        let real_key = make_key(1);
        let wrong_key = make_key(99);
        let new_key = make_key(2);

        add_repo_encrypted(&db, "r1", "Repo", "/path", "correct-password", &real_key);

        let salt = [0u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        // Rotation with wrong old key must fail and leave DB untouched.
        assert!(db.rotate_master_key(&wrong_key, &new_key, &salt, &vn, &vct).is_err());

        // Original encrypted password still readable with real_key.
        let r1 = db.get_full_repo("r1", &real_key).unwrap();
        assert_eq!(r1.password, "correct-password");
    }

    #[test]
    fn rotate_master_key_with_no_repos_still_updates_verification_row() {
        let db = test_db();
        let old_key = make_key(1);
        let new_key = make_key(2);

        let salt = [42u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        db.rotate_master_key(&old_key, &new_key, &salt, &vn, &vct).unwrap();

        // Verification row should now exist.
        let (stored_salt, _, _) = db.load_master_key_row().unwrap();
        assert_eq!(stored_salt, salt);
    }

    // ── log_backup / history trim ───────────────────────────────────────────

    fn log_entry(db: &AppDb, id: &str, started_at: i64) {
        db.log_backup(id, "repo1", None, None, started_at, 1.0, 0, 0, 0, None).unwrap();
    }

    #[test]
    fn log_backup_trims_to_history_limit() {
        let db = test_db();
        // Insert BACKUP_HISTORY_LIMIT + 1 entries (oldest first so trim is predictable).
        for i in 0..=BACKUP_HISTORY_LIMIT {
            log_entry(&db, &format!("id-{i}"), i);
        }
        let history = db.list_backup_history().unwrap();
        // Must not exceed the limit.
        assert_eq!(history.len() as i64, BACKUP_HISTORY_LIMIT);
        // Oldest entry (started_at=0) should have been trimmed.
        assert!(!history.iter().any(|e| e.id == "id-0"));
        // Newest entry must be present.
        assert!(history.iter().any(|e| e.id == format!("id-{}", BACKUP_HISTORY_LIMIT)));
    }

    #[test]
    fn log_backup_history_ordered_newest_first() {
        let db = test_db();
        log_entry(&db, "early", 100);
        log_entry(&db, "late", 200);
        let history = db.list_backup_history().unwrap();
        assert_eq!(history[0].id, "late");
        assert_eq!(history[1].id, "early");
    }

    // ── clear_cache / clean_cache ────────────────────────────────────────────

    fn seed_repo(db: &AppDb, repo_id: &str) {
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT OR IGNORE INTO repositories
                 (id, name, path, password_nonce, password_ciphertext)
                 VALUES (?1, ?2, ?3, X'', X'')",
                rusqlite::params![repo_id, repo_id, "/tmp/fake"],
            )
            .unwrap();
    }

    fn count_rows(db: &AppDb, table: &str) -> u64 {
        let conn = db.conn.lock().unwrap();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap()
    }

    #[test]
    fn clear_cache_empties_all_cache_tables() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();

        db.clear_cache().unwrap();

        assert_eq!(count_rows(&db, "snapshots_cache"), 0);
        assert_eq!(count_rows(&db, "repo_stats_cache"), 0);
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
    }

    #[test]
    fn clean_cache_removes_only_orphaned_rows() {
        let db = test_db();
        seed_repo(&db, "live-repo");
        seed_snapshot(&db, "live-repo", "aaaa111100000000");
        db.set_browse_status("live-repo", "aaaa111100000000", "complete").unwrap();

        // Seed orphaned rows: snapshot for a repo that no longer exists.
        seed_snapshot(&db, "dead-repo", "bbbb222200000000");
        db.set_browse_status("dead-repo", "bbbb222200000000", "complete").unwrap();

        let (removed, _size) = db.clean_cache().unwrap();

        // Two rows from snapshots_cache + two from browse_cache_status for
        // dead-repo should be removed (browse_cache_files had no rows, but the
        // snapshots_cache row for dead-repo is removed first, causing the
        // browse_cache_status row to be orphaned next).
        assert!(removed >= 2, "expected ≥2 orphaned rows, got {removed}");

        // Live repo's rows must still be present.
        assert_eq!(count_rows(&db, "snapshots_cache"), 1);
        assert_eq!(count_rows(&db, "browse_cache_status"), 1);
    }

    #[test]
    fn clean_cache_returns_zero_when_nothing_orphaned() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();

        let (removed, _size) = db.clean_cache().unwrap();
        assert_eq!(removed, 0);
        // All rows still present.
        assert_eq!(count_rows(&db, "snapshots_cache"), 1);
        assert_eq!(count_rows(&db, "browse_cache_status"), 1);
    }

    // ── migration regression ─────────────────────────────────────────────────

    /// Simulate an existing v0.1.0 database (user_version 0, JSON-blob cache
    /// tables) upgrading through `init_schema` and verify:
    ///
    /// 1. Persistent data (repositories, backup_plans) survives intact.
    /// 2. The old incompatible cache tables (browse_cache, snapshots_cache) are
    ///    replaced by the new relational ones.
    /// 3. PRAGMA user_version is set to 1.
    /// 4. A second call to `init_schema` is idempotent (no error).
    #[test]
    fn v0_to_v1_migration_preserves_persistent_data() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        // ── Build a v0.1.0-shaped database ──────────────────────────────────
        conn.execute_batch(
            "CREATE TABLE master_key (
                id                      INTEGER PRIMARY KEY CHECK (id = 1),
                salt                    BLOB NOT NULL,
                verification_nonce      BLOB NOT NULL,
                verification_ciphertext BLOB NOT NULL
             );
             CREATE TABLE repositories (
                id                  TEXT PRIMARY KEY,
                name                TEXT NOT NULL,
                path                TEXT NOT NULL,
                password_nonce      BLOB NOT NULL,
                password_ciphertext BLOB NOT NULL
             );
             CREATE TABLE backup_plans (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                repo_id         TEXT NOT NULL,
                paths_json      TEXT NOT NULL,
                tags_json       TEXT NOT NULL,
                excludes_json   TEXT NOT NULL,
                retention_json  TEXT
             );
             CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             -- v0.1.0 JSON-blob cache tables (incompatible with v0.1.1 schema):
             CREATE TABLE browse_cache (
                snapshot_id  TEXT NOT NULL,
                path         TEXT NOT NULL,
                entries_json TEXT NOT NULL,
                cached_at    INTEGER NOT NULL,
                PRIMARY KEY (snapshot_id, path)
             );
             CREATE TABLE snapshots_cache (
                repo_id        TEXT PRIMARY KEY,
                snapshots_json TEXT NOT NULL,
                cached_at      INTEGER NOT NULL
             );
             -- user_version left at 0 (default) — no PRAGMA set",
        )
        .unwrap();

        // Seed persistent rows that must survive migration.
        conn.execute(
            "INSERT INTO repositories (id, name, path, password_nonce, password_ciphertext)
             VALUES ('repo-sentinel', 'My Repo', '/backups', X'', X'')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO backup_plans (id, name, repo_id, paths_json, tags_json, excludes_json)
             VALUES ('plan-sentinel', 'Daily', 'repo-sentinel', '[\"/home\"]', '[]', '[]')",
            [],
        )
        .unwrap();
        // Seed a stale cache row that should be dropped.
        conn.execute(
            "INSERT INTO browse_cache (snapshot_id, path, entries_json, cached_at)
             VALUES ('oldsnap', '/', '[]', 0)",
            [],
        )
        .unwrap();

        // ── Run migration ────────────────────────────────────────────────────
        AppDb::init_schema(&conn).unwrap();

        // 1. user_version bumped to the latest (2): starting from a fresh v0 DB,
        // both the v0→v1 and v1→v2 migration blocks run in the same call.
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);

        // 2. Old cache tables are gone.
        let old_browse: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='browse_cache'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_browse, 0, "old browse_cache table should be dropped");

        // The old snapshots_cache (repo_id PK, snapshots_json) is gone.
        // The new one (repo_id, snapshot_id, ...) now exists; verify its shape
        // by confirming the new column is present.
        let new_sc: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='snapshots_cache'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_sc, 1, "new snapshots_cache table should exist");

        // New relational cache tables exist.
        let bcf: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='browse_cache_files'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bcf, 1, "browse_cache_files should exist");

        let bcs: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='browse_cache_status'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bcs, 1, "browse_cache_status should exist");

        let isn: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='indexed_snapshots'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(isn, 1, "indexed_snapshots should exist");

        // browse_cache_files no longer carries name/cached_at — snapshot_id is
        // now interned via indexed_snapshots.snap.
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(browse_cache_files)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(cols.contains(&"snap".to_string()));
        assert!(!cols.contains(&"snapshot_id".to_string()));
        assert!(!cols.contains(&"name".to_string()));
        assert!(!cols.contains(&"cached_at".to_string()));

        // 3. Persistent data survived.
        let repo_name: String = conn
            .query_row(
                "SELECT name FROM repositories WHERE id = 'repo-sentinel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(repo_name, "My Repo");

        let plan_name: String = conn
            .query_row(
                "SELECT name FROM backup_plans WHERE id = 'plan-sentinel'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(plan_name, "Daily");

        // 4. Idempotent — second call must not error.
        AppDb::init_schema(&conn).unwrap();
        let version2: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version2, 2);
    }

    /// Covers the Quick-wins browse-cache rewrite: insert two snapshots that
    /// share one file path, and verify every reader/deleter behaves correctly
    /// against the interned-snapshot schema (name recomputed from path,
    /// snapshot_id resolved via indexed_snapshots, per-snapshot isolation).
    #[test]
    fn test_browse_cache_dedup_round_trip() {
        let db = test_db();
        let repo_id = "repo-a";
        seed_snapshot(&db, repo_id, "snap1");
        seed_snapshot(&db, repo_id, "snap2");

        let shared = FileEntry {
            name: "shared.txt".to_string(),
            path: "/shared.txt".to_string(),
            entry_type: "file".to_string(),
            size: Some(42),
            mtime: Some("2024-01-01T00:00:00Z".to_string()),
            mode: Some(0o644),
        };
        let only_in_snap1 = FileEntry {
            name: "only1.txt".to_string(),
            path: "/only1.txt".to_string(),
            entry_type: "file".to_string(),
            size: Some(7),
            mtime: None,
            mode: None,
        };

        db.insert_browse_files("snap1", &[shared.clone(), only_in_snap1.clone()])
            .unwrap();
        db.insert_browse_files("snap2", std::slice::from_ref(&shared)).unwrap();
        db.set_browse_status(repo_id, "snap1", "complete").unwrap();
        db.set_browse_status(repo_id, "snap2", "complete").unwrap();

        // get(): directory listing recomputes `name` from `path` correctly.
        let listing = db.get(repo_id, "snap1", None).unwrap().unwrap();
        assert_eq!(listing.len(), 2);
        assert!(listing.iter().any(|e| e.name == "shared.txt"));
        assert!(listing.iter().any(|e| e.name == "only1.txt"));

        // search_browse_files(): single-snapshot search, name derived correctly,
        // and the (now dropped) `name` column isn't needed to match.
        let hits = db.search_browse_files("snap1", "only1", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "only1.txt");

        // A snapshot id that was never indexed is a clean miss, not an error.
        assert!(db.search_browse_files("never-indexed", "shared", 10).unwrap().is_empty());
        assert!(db.get(repo_id, "never-indexed", None).unwrap().is_none());

        // evict(): removing snap1 doesn't touch snap2's rows, and its
        // indexed_snapshots row is cleaned up (re-indexing snap1 later would
        // intern a fresh row rather than reuse a stale one).
        db.evict(repo_id, "snap1").unwrap();
        assert!(db.get(repo_id, "snap1", None).unwrap().is_none());
        let snap2_listing = db.get(repo_id, "snap2", None).unwrap().unwrap();
        assert_eq!(snap2_listing.len(), 1);
        assert_eq!(snap2_listing[0].name, "shared.txt");
    }

    #[test]
    fn get_snapshots_vec_matches_the_former_json_round_trip() {
        // get_snapshots_vec() replaced a get_snapshots()->JSON string ->
        // serde_json::from_str::<Vec<Snapshot>> round trip in list_snapshots. This
        // asserts the direct-struct path produces the same data the old round trip did.
        let db = test_db();
        let repo_id = "repoA";
        let json = r#"[
            {"id":"snap-a00000000000","short_id":"snapa000","time":"2024-01-01T00:00:00Z","hostname":"host1","username":"alice","paths":["/home/alice"],"tags":["daily","weekly"]},
            {"id":"snap-b00000000000","short_id":"snapb000","time":"2024-02-01T00:00:00Z","hostname":"host2","paths":["/home/bob"]}
        ]"#;
        db.set_snapshots(repo_id, json).unwrap();

        let snapshots = db.get_snapshots_vec(repo_id).unwrap();
        assert_eq!(snapshots.len(), 2);

        // set_snapshots doesn't guarantee row order matches insertion order beyond
        // the ORDER BY time ASC in get_snapshots_vec's query, so assert on IDs.
        let a = snapshots.iter().find(|s| s.id == "snap-a00000000000").unwrap();
        assert_eq!(a.short_id, "snapa000");
        assert_eq!(a.hostname, "host1");
        assert_eq!(a.username.as_deref(), Some("alice"));
        assert_eq!(a.paths, vec!["/home/alice".to_string()]);
        assert_eq!(a.tags, Some(vec!["daily".to_string(), "weekly".to_string()]));

        let b = snapshots.iter().find(|s| s.id == "snap-b00000000000").unwrap();
        assert_eq!(b.hostname, "host2");
        assert!(b.username.is_none());
        assert!(b.tags.is_none());

        // A repo with no cached rows returns an empty Vec, not an error — matches
        // the old get_snapshots() `None` -> `Ok(vec![])` fallback in list_snapshots.
        assert!(db.get_snapshots_vec("no-such-repo").unwrap().is_empty());
    }

    #[test]
    fn v1_to_v2_migration_does_not_vacuum() {
        // Build a DB in the v1 state: user_version=1 and v1-shaped browse cache.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 1;")
            .expect("set user_version to 1");

        // Create v1-shaped browse_cache_files (old schema: snapshot_id TEXT, name, per-row cached_at).
        conn.execute_batch(
            "CREATE TABLE browse_cache_files (
                 snapshot_id TEXT,
                 path TEXT,
                 parent_path TEXT,
                 name TEXT,
                 entry_type TEXT,
                 size INTEGER,
                 mtime INTEGER,
                 mode INTEGER
             );
             CREATE TABLE browse_cache_status (
                 repo_id TEXT,
                 snapshot_id TEXT,
                 status TEXT,
                 cached_at INTEGER,
                 PRIMARY KEY (repo_id, snapshot_id)
             );",
        )
        .expect("create v1 tables");

        // Populate enough rows to span multiple pages so freelist_count is clearly > 0 after DROP.
        let mut stmt = conn
            .prepare(
                "INSERT INTO browse_cache_files (snapshot_id, path, parent_path, name, entry_type, size, mtime, mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .expect("prepare insert");
        for i in 0..2000 {
            let snap_id = format!("{:064x}", i);
            let path = format!("/some/deep/path/to/file_{:04}.txt", i);
            stmt.execute((
                snap_id.as_str(),
                path.as_str(),
                "/some/deep/path/to",
                "file",
                "file",
                1234i64,
                0i64,
                0i64,
            ))
            .expect("insert row");
        }

        // Run init_schema — this should perform the v1→v2 migration (but NOT vacuum).
        AppDb::init_schema(&conn).expect("init_schema v1→v2 migration");

        // user_version must be bumped to 2.
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("query user_version");
        assert_eq!(version, 2, "user_version must be 2 after v1→v2 migration");

        // browse_cache_files must have the v2 schema (interned `snap` column, no `name`).
        let mut table_info = conn
            .prepare("PRAGMA table_info(browse_cache_files)")
            .expect("prepare table_info");
        let has_snap_column = table_info
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query column names")
            .any(|name_res| matches!(name_res, Ok(name) if name == "snap"));
        assert!(
            has_snap_column,
            "v2 browse_cache_files must have the interned `snap` column"
        );

        // Critical regression guard: VACUUM must NOT have run. The dropped-table pages
        // should still be on the freelist (freelist_count > 0). A VACUUM would have
        // reclaimed them to ~0 and shrunk the file.
        let freelist_count: i64 = conn
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .expect("query freelist_count");
        assert!(
            freelist_count > 0,
            "freelist_count must be > 0 after migration (no VACUUM); got {}",
            freelist_count
        );
    }
}
