use std::sync::{Arc, Mutex};
use std::str::FromStr;

use chrono::Local;
use cron::Schedule as CronSchedule;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::browse::FileEntry;
use super::crypto;

/// Max rows retained in `backup_history`. Read and trim both use this so they
/// never drift — the Logs page never shows rows the trim would have deleted.
const BACKUP_HISTORY_LIMIT: i64 = 1000;

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

#[derive(ZeroizeOnDrop)]
pub struct FullRepository {
    #[zeroize(skip)]
    pub path: String,
    pub password: String,
}

// ── copy cancellation handle ──────────────────────────────────────────────

pub struct CopyHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl CopyHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

pub struct MirrorHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl MirrorHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
}

impl BackupHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            busy: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

pub struct PruneHandle {
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl PruneHandle {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
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
            CREATE TABLE IF NOT EXISTS browse_cache_files (
                snapshot_id  TEXT NOT NULL,
                path         TEXT NOT NULL,
                parent_path  TEXT NOT NULL,
                name         TEXT NOT NULL,
                entry_type   TEXT NOT NULL,
                size         INTEGER,
                mtime        TEXT,
                mode         INTEGER,
                cached_at    INTEGER NOT NULL,
                PRIMARY KEY (snapshot_id, path)
            );
            CREATE INDEX IF NOT EXISTS idx_browse_files
                ON browse_cache_files (snapshot_id, parent_path);
            CREATE TABLE IF NOT EXISTS browse_cache_status (
                repo_id      TEXT NOT NULL,
                snapshot_id  TEXT NOT NULL,
                status       TEXT NOT NULL DEFAULT 'pending',
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

    pub fn load_master_key_row(&self) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
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
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM repositories WHERE id = ?1", params![repo_id])
            .map_err(|e| e.to_string())?;

        // Cascade cleanup: remove browse cache entries for this repo's snapshots.
        // browse_cache_status is keyed by (repo_id, snapshot_id).
        conn.execute(
            "DELETE FROM browse_cache_status WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        // browse_cache_files is keyed by (snapshot_id, path) without repo_id,
        // so we delete all files for snapshots that belong to this repo.
        conn.execute(
            "DELETE FROM browse_cache_files WHERE snapshot_id IN (SELECT snapshot_id FROM snapshots_cache WHERE repo_id = ?1)",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "DELETE FROM snapshots_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "DELETE FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;

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
                        .map(|s| serde_json::from_str(s))
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
            .map(|r| serde_json::to_string(r))
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

    fn is_fully_indexed(&self, repo_id: &str, snapshot_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT 1 FROM browse_cache_status WHERE repo_id = ?1 AND snapshot_id = ?2 AND status = 'complete'",
            params![repo_id, snapshot_id],
            |_| Ok(()),
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn get(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        path: Option<&str>,
    ) -> Result<Option<Vec<FileEntry>>, String> {
        let fully_indexed = self.is_fully_indexed(repo_id, snapshot_id)?;
        let parent_key = path.unwrap_or("");
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT name, path, entry_type, size, mtime, mode
                 FROM browse_cache_files
                 WHERE snapshot_id = ?1 AND parent_path = ?2",
            )
            .map_err(|e| e.to_string())?;
        let entries = stmt
            .query_map(params![snapshot_id, parent_key], |row| {
                Ok(FileEntry {
                    name: row.get(0)?,
                    path: row.get(1)?,
                    entry_type: row.get(2)?,
                    size: row.get(3)?,
                    mtime: row.get(4)?,
                    mode: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

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
        let now = timestamp();
        let parent_key = path.unwrap_or("");
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM browse_cache_files WHERE snapshot_id = ?1 AND parent_path = ?2",
            params![snapshot_id, parent_key],
        )
        .map_err(|e| e.to_string())?;
        for entry in entries {
            conn.execute(
                "INSERT OR REPLACE INTO browse_cache_files
                 (snapshot_id, path, parent_path, name, entry_type, size, mtime, mode, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    snapshot_id,
                    entry.path,
                    parent_key,
                    entry.name,
                    entry.entry_type,
                    entry.size,
                    entry.mtime,
                    entry.mode,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn evict(&self, repo_id: &str, snapshot_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM browse_cache_files WHERE snapshot_id = ?1",
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
            .prepare(
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
            "INSERT OR REPLACE INTO browse_cache_status (repo_id, snapshot_id, status)
             VALUES (?1, ?2, ?3)",
            params![repo_id, snapshot_id, status],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Full-text substring search across all indexed files in a snapshot.
    /// Matches against both the file name and the full path.
    pub fn search_browse_files(
        &self,
        snapshot_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<FileEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // Escape LIKE metacharacters in the user's query so they're treated literally.
        let pattern = format!("%{}%", query.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_"));
        let mut stmt = conn
            .prepare(
                "SELECT name, path, entry_type, size, mtime, mode
                 FROM browse_cache_files
                 WHERE snapshot_id = ?1
                   AND (name LIKE ?2 ESCAPE '\\' OR path LIKE ?2 ESCAPE '\\')
                 ORDER BY path
                 LIMIT ?3",
            )
            .map_err(|e| e.to_string())?;
        let entries = stmt
            .query_map(params![snapshot_id, pattern, limit as i64], |row| {
                Ok(FileEntry {
                    name: row.get(0)?,
                    path: row.get(1)?,
                    entry_type: row.get(2)?,
                    size: row.get(3)?,
                    mtime: row.get(4)?,
                    mode: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(entries)
    }

    /// Bulk-insert file entries for a snapshot (used by the cache warmer and manual indexing).
    /// Inserts in chunks of 500 to avoid holding the mutex for excessive time.
    pub fn insert_browse_files(
        &self,
        snapshot_id: &str,
        entries: &[FileEntry],
    ) -> Result<(), String> {
        let now = timestamp();
        for chunk in entries.chunks(500) {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            for entry in chunk {
                let parent = parent_path_of(&entry.path);
                tx.execute(
                    "INSERT OR REPLACE INTO browse_cache_files
                     (snapshot_id, path, parent_path, name, entry_type, size, mtime, mode, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        snapshot_id,
                        entry.path,
                        parent,
                        entry.name,
                        entry.entry_type,
                        entry.size,
                        entry.mtime,
                        entry.mode,
                        now
                    ],
                )
                .map_err(|e| e.to_string())?;
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

    // ── snapshots cache ──────────────────────────────────────────────────────

    pub fn get_snapshots(&self, repo_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT snapshot_id, short_id, time, hostname, username, paths, tags
                 FROM snapshots_cache WHERE repo_id = ?1
                 ORDER BY time ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![repo_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        if rows.is_empty() {
            return Ok(None);
        }

        let values: Vec<serde_json::Value> = rows
            .into_iter()
            .map(|(id, short_id, time, hostname, username, paths, tags)| {
                let mut obj = serde_json::json!({
                    "id": id,
                    "short_id": short_id,
                    "time": time,
                    "hostname": hostname,
                    "paths": serde_json::from_str::<serde_json::Value>(&paths)
                        .unwrap_or(serde_json::Value::Array(vec![])),
                });
                if let Some(u) = username {
                    obj["username"] = serde_json::Value::String(u);
                }
                if let Some(t) = tags {
                    obj["tags"] = serde_json::from_str::<serde_json::Value>(&t)
                        .unwrap_or(serde_json::Value::Null);
                }
                obj
            })
            .collect();

        serde_json::to_string(&values)
            .map(Some)
            .map_err(|e| e.to_string())
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
        for s in &rows {
            tx.execute(
                "INSERT INTO snapshots_cache
                 (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    repo_id,
                    s.id,
                    s.short_id,
                    s.time,
                    s.hostname,
                    s.username,
                    s.paths,
                    s.tags,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Upsert-only: inserts new snapshot rows without clearing existing ones.
    /// Used by execute_backup to add a newly created snapshot to the cache.
    pub fn append_snapshots(&self, repo_id: &str, json: &str) -> Result<(), String> {
        let rows = parse_snapshot_rows(json)?;
        let now = timestamp();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        for s in &rows {
            conn.execute(
                "INSERT OR REPLACE INTO snapshots_cache
                 (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, cached_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    repo_id,
                    s.id,
                    s.short_id,
                    s.time,
                    s.hostname,
                    s.username,
                    s.paths,
                    s.tags,
                    now
                ],
            )
            .map_err(|e| e.to_string())?;
        }
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

    pub fn get_stats(&self, repo_id: &str) -> Result<Option<(u64, u64, u64)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT total_size, total_file_count, snapshots_count
             FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, i64>(2)? as u64,
                ))
            },
        ) {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set_stats(
        &self,
        repo_id: &str,
        total_size: u64,
        total_file_count: u64,
        snapshots_count: u64,
    ) -> Result<(), String> {
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
        Ok(())
    }

    pub fn evict_stats(&self, repo_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── backup history ────────────────────────────────────────────────────────

    pub fn list_backup_history(&self) -> Result<Vec<BackupHistoryEntry>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
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
        // without bound. Runs after the insert is already persisted.
        conn.execute(
            "DELETE FROM backup_history WHERE id NOT IN (
                 SELECT id FROM backup_history ORDER BY started_at DESC LIMIT ?1
             )",
            params![BACKUP_HISTORY_LIMIT],
        )
        .map_err(|e| e.to_string())?;
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
                        .map(|s| serde_json::from_str(s))
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?,
                    limit_upload,
                    limit_download,
                })
            })
            .collect()
    }

    // ── global clear ─────────────────────────────────────────────────────────

    pub fn clear_cache(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "DELETE FROM browse_cache_files;
             DELETE FROM browse_cache_status;
             DELETE FROM snapshots_cache;
             DELETE FROM repo_stats_cache;",
        )
        .map_err(|e| e.to_string())?;
        // VACUUM rewrites all live pages into the WAL (in WAL mode). Checkpoint
        // afterwards moves those compacted pages into the main file and truncates
        // the WAL, so both files end up small.
        conn.execute_batch("VACUUM;").map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())?;
        // Read the file sizes while still holding the connection lock so no
        // background thread can write a new WAL frame before we sample the size.
        let main = std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
        let wal = std::fs::metadata(self.db_path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(main + wal)
    }

    /// Remove only orphaned cache rows, leaving live caches intact. Returns the
    /// number of rows deleted. Orphans are:
    ///   - `snapshots_cache` / `repo_stats_cache` rows whose `repo_id` no longer
    ///     exists in `repositories` (e.g. a deleted repo),
    ///   - `browse_cache_files` / `browse_cache_status` rows whose `snapshot_id`
    ///     is not referenced by any remaining `snapshots_cache` entry.
    pub fn clean_cache(&self) -> Result<u64, String> {
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
        Ok(removed)
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
pub fn clear_browse_cache(db: tauri::State<'_, AppDb>) -> Result<u64, String> {
    db.clear_cache()
}

#[tauri::command]
pub fn clean_cache(db: tauri::State<'_, AppDb>) -> Result<u64, String> {
    db.clean_cache()
}

#[tauri::command]
pub fn get_db_size(app: tauri::AppHandle) -> Result<u64, String> {
    use tauri::Manager;
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("app_data.db");
    let main = std::fs::metadata(&base).map(|m| m.len()).unwrap_or(0);
    let wal = std::fs::metadata(base.with_extension("db-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(main + wal)
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
        assert!(status_a.get("snap123").is_none());

        // Verify repoB's status remains.
        let status_b = db.get_browse_status("repoB").unwrap();
        assert_eq!(status_b.get("snap123"), Some(&"complete".to_string()));
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
}
