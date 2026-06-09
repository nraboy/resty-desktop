use std::sync::Mutex;

use rusqlite::{params, Connection};

use super::browse::FileEntry;

pub struct SnapshotCache {
    conn: Mutex<Connection>,
}

impl SnapshotCache {
    pub fn new(conn: Connection) -> Self {
        Self {
            conn: Mutex::new(conn),
        }
    }

    pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS browse_cache (
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
            );",
        )
    }

    // --- browse cache ---

    pub fn get(&self, snapshot_id: &str, path: Option<&str>) -> Result<Option<Vec<FileEntry>>, String> {
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

    pub fn set(&self, snapshot_id: &str, path: Option<&str>, entries: &[FileEntry]) -> Result<(), String> {
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

    // --- snapshots list cache ---

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

    // --- global clear ---

    pub fn clear(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute_batch("DELETE FROM browse_cache; DELETE FROM snapshots_cache;")
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
pub fn clear_browse_cache(cache: tauri::State<'_, SnapshotCache>) -> Result<(), String> {
    cache.clear()
}
