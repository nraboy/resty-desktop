use std::sync::{Arc, Mutex};
use std::str::FromStr;

use chrono::Local;
use cron::Schedule as CronSchedule;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::browse::FileEntry;
use super::crypto;

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

pub struct FullRepository {
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
        *self.0.lock().map_err(|e| e.to_string())? = Some(key);
        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }
}

// ── database ───────────────────────────────────────────────────────────────

pub struct AppDb {
    conn: Mutex<Connection>,
}

impl AppDb {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
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
                id             TEXT PRIMARY KEY,
                name           TEXT NOT NULL,
                repo_id        TEXT NOT NULL,
                paths_json     TEXT NOT NULL,
                tags_json      TEXT NOT NULL,
                excludes_json  TEXT NOT NULL,
                retention_json TEXT
            );
            CREATE TABLE IF NOT EXISTS app_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS browse_cache (
                snapshot_id  TEXT NOT NULL,
                path         TEXT NOT NULL,
                entries_json TEXT NOT NULL,
                cached_at    INTEGER NOT NULL,
                PRIMARY KEY (snapshot_id, path)
            );
            CREATE TABLE IF NOT EXISTS snapshots_cache (
                repo_id        TEXT PRIMARY KEY,
                snapshots_json TEXT NOT NULL,
                cached_at      INTEGER NOT NULL
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
        )
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

    pub fn reencrypt_repo_passwords(
        &self,
        old_key: &[u8; 32],
        new_key: &[u8; 32],
    ) -> Result<(), String> {
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare("SELECT id, password_nonce, password_ciphertext FROM repositories")
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };

        let re_encrypted: Vec<(String, Vec<u8>, Vec<u8>)> = rows
            .into_iter()
            .map(|(id, nonce, ct)| {
                let pw = crypto::decrypt(old_key, &nonce, &ct)?;
                let (new_nonce, new_ct) = crypto::encrypt(new_key, &pw)?;
                Ok((id, new_nonce, new_ct))
            })
            .collect::<Result<Vec<_>, String>>()?;

        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (id, nonce, ct) in re_encrypted {
            tx.execute(
                "UPDATE repositories SET password_nonce = ?1, password_ciphertext = ?2 WHERE id = ?3",
                params![nonce, ct, id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── backup plans ────────────────────────────────────────────────────────

    pub fn list_backup_plans(&self) -> Result<Vec<BackupPlan>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, repo_id, paths_json, tags_json, excludes_json, retention_json FROM backup_plans ORDER BY rowid")
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
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        plans
            .into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, retention_json)| {
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
             (id, name, repo_id, paths_json, tags_json, excludes_json, retention_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                plan.id,
                plan.name,
                plan.repo_id,
                paths_json,
                tags_json,
                excludes_json,
                retention_json
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

    pub fn get(
        &self,
        snapshot_id: &str,
        path: Option<&str>,
    ) -> Result<Option<Vec<FileEntry>>, String> {
        let path_key = path.unwrap_or("");
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT entries_json FROM browse_cache WHERE snapshot_id = ?1 AND path = ?2",
            params![snapshot_id, path_key],
            |row| row.get::<_, String>(0),
        ) {
            Ok(json) => serde_json::from_str(&json).map(Some).map_err(|e| e.to_string()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(
        &self,
        snapshot_id: &str,
        path: Option<&str>,
        entries: &[FileEntry],
    ) -> Result<(), String> {
        let path_key = path.unwrap_or("");
        let json = serde_json::to_string(entries).map_err(|e| e.to_string())?;
        let now = timestamp();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO browse_cache (snapshot_id, path, entries_json, cached_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![snapshot_id, path_key, json, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn evict(&self, snapshot_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM browse_cache WHERE snapshot_id = ?1",
            params![snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── snapshots cache ──────────────────────────────────────────────────────

    pub fn get_snapshots(&self, repo_id: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT snapshots_json FROM snapshots_cache WHERE repo_id = ?1",
            params![repo_id],
            |row| row.get::<_, String>(0),
        ) {
            Ok(json) => Ok(Some(json)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set_snapshots(&self, repo_id: &str, json: &str) -> Result<(), String> {
        let now = timestamp();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO snapshots_cache (repo_id, snapshots_json, cached_at)
             VALUES (?1, ?2, ?3)",
            params![repo_id, json, now],
        )
        .map_err(|e| e.to_string())?;
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
                 LIMIT 500",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
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
            "SELECT id, name, repo_id, paths_json, tags_json, excludes_json, retention_json
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
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, retention_json)| {
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
                })
            })
            .collect()
    }

    // ── global clear ─────────────────────────────────────────────────────────

    pub fn clear_cache(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "DELETE FROM browse_cache;
             DELETE FROM snapshots_cache;
             DELETE FROM repo_stats_cache;",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
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
             DELETE FROM browse_cache;
             DELETE FROM snapshots_cache;
             DELETE FROM repo_stats_cache;
             DELETE FROM backup_history;
             DELETE FROM schedules;
             COMMIT;",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn clear_browse_cache(db: tauri::State<'_, AppDb>) -> Result<(), String> {
    db.clear_cache()
}

#[tauri::command]
pub fn list_backup_history(db: tauri::State<'_, AppDb>) -> Result<Vec<BackupHistoryEntry>, String> {
    db.list_backup_history()
}
