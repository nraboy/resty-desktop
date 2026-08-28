use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use zeroize::{Zeroize, ZeroizeOnDrop};

use super::browse::FileEntry;
use super::crypto;
use super::repo::ResticStats;
use super::snapshot::Snapshot;
use crate::tasks::{emit_cancelling, new_task_slot, OperationCtx, TaskKind, TaskOrigin, TaskProgress, TaskSlot};

/// Max rows retained in `backup_history`. Read and trim both use this so they
/// never drift — the Logs page never shows rows the trim would have deleted.
const BACKUP_HISTORY_LIMIT: i64 = 1000;

/// Rows deleted per `drain_orphans` call by the synchronous, single-call
/// `AppDb::clean_cache` (tests, and any other non-`spawn_blocking` caller). The
/// `clean_cache` Tauri command uses the same batch size for its own incremental loop
/// — see `commands/cache.rs`'s command-level `clean_cache` for why batching matters.
const CLEAN_CACHE_BATCH_ROWS: usize = 5_000;

/// One `drain_orphans` call's result. `more_remaining` tells the caller whether to
/// call again.
pub struct DrainBatch {
    pub rows_deleted: u64,
    pub more_remaining: bool,
}

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
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub path: String,
    pub read_only: bool,
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

/// Which backup lifecycle stage a webhook triggers on. `Completed` deliberately has no
/// changed/unchanged split — that distinction belongs to the OS notification categories
/// (`notify::classify_success`), not to webhooks.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WebhookStage {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebhookStages {
    #[serde(default)]
    pub started: bool,
    #[serde(default = "default_true")]
    pub completed: bool,
    #[serde(default = "default_true")]
    pub failed: bool,
}

impl Default for WebhookStages {
    fn default() -> Self {
        Self { started: false, completed: true, failed: true }
    }
}

impl WebhookStages {
    pub fn wants(&self, stage: WebhookStage) -> bool {
        match stage {
            WebhookStage::Started => self.started,
            WebhookStage::Completed => self.completed,
            WebhookStage::Failed => self.failed,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WebhookProvider {
    Generic,
    Discord,
    Slack,
    /// Microsoft Teams — the fixed Adaptive Card wrapper a Power Automate Workflows
    /// webhook URL expects, with `build_message`'s text nested in a TextBlock.
    Teams,
    /// User-authored JSON body template with {placeholders} (see webhook.rs's
    /// `interpolate`); the presets ignore `PlanWebhook::template`.
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanWebhook {
    pub id: String,
    pub url: String,
    pub provider: WebhookProvider,
    #[serde(default)]
    pub stages: WebhookStages,
    /// Custom-provider JSON body template with {placeholders}; ignored by the presets.
    /// Additive field inside `webhooks_json` — no schema change.
    #[serde(default)]
    pub template: Option<String>,
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
    #[serde(default)]
    pub exclude_if_present: Vec<String>,
    #[serde(default)]
    pub exclude_caches: bool,
    pub retention: Option<RetentionPolicy>,
    pub limit_upload: Option<u32>,
    pub limit_download: Option<u32>,
    #[serde(default)]
    pub webhooks: Vec<PlanWebhook>,
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

type ScheduleRow = (String, String, String, String, i64, Option<i64>, Option<i64>, i64);

/// Shared row→`Schedule` mapping for `list_schedules` and `get_schedule` — kept as one
/// function so the two never drift on which column is which.
fn row_to_schedule(row: ScheduleRow) -> Result<Schedule, String> {
    let (id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at) = row;
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
}

// ── internal type (never serialised) ───────────────────────────────────────

/// One backend credential (e.g. `AWS_ACCESS_KEY_ID`) carried on a `FullRepository`.
/// A named struct rather than a `(String, String)` tuple because tuples don't
/// implement `Zeroize` — this needs the key kept (not secret) but the value wiped.
#[derive(Clone, ZeroizeOnDrop)]
pub struct Credential {
    #[zeroize(skip)]
    pub key: String,
    pub value: String,
}

#[derive(Clone, ZeroizeOnDrop)]
pub struct FullRepository {
    #[zeroize(skip)]
    pub path: String,
    pub password: String,
    /// Mirrors `Repository::read_only` — carried here so every restic call site that
    /// already holds a `FullRepository` can apply `--no-lock` without a second DB lookup.
    /// See `repo::apply_repo_flags`.
    #[zeroize(skip)]
    pub read_only: bool,
    /// Backend credentials (e.g. B2_ACCOUNT_ID/KEY, AWS_ACCESS_KEY_ID/SECRET), decrypted
    /// under the master key alongside `password`. Empty means "use restic's own
    /// credential chain" (env inherited from the app process, ~/.aws/credentials, IAM
    /// role, …) — the ambient mode every pre-existing repo is in and stays in unless the
    /// user explicitly stores credentials. See `repo::apply_backend_env` and CLAUDE.md's
    /// "Backend credentials" section.
    pub credentials: Vec<Credential>,
}

/// A (nonce, ciphertext) pair for an optional encrypted blob — `None` for both means
/// "nothing stored" (NULL in the `repositories` table), not "an encrypted empty value".
type OptionalSecretBlob = (Option<Vec<u8>>, Option<Vec<u8>>);

/// One `repositories` row's raw secret columns as read from SQLite, before decryption.
type RepoSecretRow = (Vec<u8>, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Same as `RepoSecretRow`, with the row's id alongside — used where the caller needs
/// to write back per-row (e.g. `rotate_master_key`'s re-encrypt loop).
type RepoSecretRowWithId = (String, Vec<u8>, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

/// Serializes a credential list to an encrypted JSON blob, or `(None, None)` for an
/// empty list — the ambient-mode encoding every pre-existing repo row already has.
/// Never write an encrypted *empty* map: that would be a second, distinct encoding of
/// "no credentials" alongside NULL, and every reader would need to handle both.
pub(crate) fn encode_credentials(
    key: &[u8; 32],
    credentials: &[Credential],
) -> Result<OptionalSecretBlob, String> {
    if credentials.is_empty() {
        return Ok((None, None));
    }
    let map: HashMap<&str, &str> =
        credentials.iter().map(|c| (c.key.as_str(), c.value.as_str())).collect();
    let mut json = serde_json::to_vec(&map).map_err(|e| e.to_string())?;
    let (nonce, ciphertext) = crypto::encrypt(key, &json)?;
    json.zeroize();
    Ok((Some(nonce), Some(ciphertext)))
}

/// The single shared decrypt+parse path for a repo's secrets — used by both
/// `get_full_repo` and `rotate_master_key`'s post-rotation verification pass, which
/// is what makes that guard generic across any secret field added here in the
/// future (see `rotate_master_key`'s doc comment).
fn decode_secrets(
    key: &[u8; 32],
    password_nonce: &[u8],
    password_ciphertext: &[u8],
    credentials_nonce: Option<&[u8]>,
    credentials_ciphertext: Option<&[u8]>,
) -> Result<(String, Vec<Credential>), String> {
    let password_bytes = crypto::decrypt(key, password_nonce, password_ciphertext)?;
    let password = String::from_utf8(password_bytes).map_err(|e| e.to_string())?;

    let credentials = match (credentials_nonce, credentials_ciphertext) {
        (Some(cn), Some(cc)) => {
            let mut json = crypto::decrypt(key, cn, cc)?;
            let map: HashMap<String, String> =
                serde_json::from_slice(&json).map_err(|e| e.to_string())?;
            json.zeroize();
            map.into_iter().map(|(key, value)| Credential { key, value }).collect()
        }
        _ => Vec::new(),
    };

    Ok((password, credentials))
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

/// Coordinates queued mirror runs so multiple `mirror_repo` calls (e.g. several
/// sources into the same destination, or entirely different repo pairs) can be
/// submitted at once without corrupting each other's state, while restic itself
/// only ever runs one `copy` process at a time.
///
/// Modeled directly on `IndexHandle`'s "Index All" batch machinery
/// (`IndexHandle::batch_turn`/`batches`), but simpler: mirror has no `gate`
/// equivalent (a single `restic copy` process has nothing to memory-bound the
/// way concurrent `restic ls` calls did — see CLAUDE.md's `IndexHandle::gate`
/// note) and no per-item loop (a mirror is one process, not N snapshots).
pub struct MirrorHandle {
    /// FIFO lane — exactly one mirror actually runs (has a live child) at a time.
    /// A mirror waiting on this is "queued"; tokio's `Mutex` is FIFO among
    /// waiters, so queued mirrors run in (approximately, for human-paced clicks)
    /// the order they were submitted — same tolerance `IndexHandle::batch_turn`
    /// documents.
    pub turn: Arc<tokio::sync::Mutex<()>>,
    /// Per-mirror cancel/child/slot registry, keyed by operationId, so
    /// concurrently queued/running mirrors (including two into the same
    /// destination from different sources) can each be tracked and cancelled
    /// independently — mirrors `IndexHandle::batches`.
    pub mirrors: Arc<Mutex<HashMap<String, MirrorEntry>>>,
}

impl MirrorHandle {
    pub fn new() -> Self {
        Self {
            turn: Arc::new(tokio::sync::Mutex::new(())),
            mirrors: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// One mirror run's cancel flag/child/task slot, registered in
/// `MirrorHandle::mirrors` for the run's duration (queued through terminal).
/// `cancel_mirror(operation_id)` looks this up to target exactly one run.
/// Mirrors `BatchCancel` (browse.rs's index-batch equivalent).
#[derive(Clone)]
pub struct MirrorEntry {
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub task_slot: TaskSlot,
    /// Wakes a mirror that's parked waiting for its turn on `MirrorHandle::turn`
    /// so it cancels immediately instead of waiting for the mirror ahead of it
    /// to finish. `cancel_mirror` calls `notify_one()` right after setting
    /// `cancel`; the run's `tokio::select!` between this and `turn.lock()`
    /// picks up whichever fires first. `notify_one`'s stored-permit semantics
    /// make this race-free even if the notify arrives before the run starts
    /// waiting. Mirrors `BatchCancel::cancel_notify`.
    pub cancel_notify: Arc<tokio::sync::Notify>,
    /// False while the run is still queued waiting for `MirrorHandle::turn`
    /// (registered but not yet `activate()`d); flipped true the moment it wins
    /// its turn and actually starts copying. Mirrors `BatchCancel::started`.
    pub started: Arc<std::sync::atomic::AtomicBool>,
    /// The live child process, `Some` only while this run actually holds
    /// `turn` and is executing `restic copy`. `None` while merely queued.
    pub child: Arc<Mutex<Option<std::process::Child>>>,
    /// Source/destination repo ids this run copies between — used by
    /// `mirror_repo`'s `(src, dest)` dedup guard, and to run `restic unlock`
    /// against both repos after a cancel-kill.
    pub src_id: String,
    pub dest_id: String,
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

/// App-state handle for the "Clean Orphaned Data" button (`clean_cache`). Unlike
/// every other handle in this file, cleanup never shells out to restic — there's no
/// child process to track or kill, so `cancelled` alone is the entire cancel path,
/// checked between `drain_orphans` batches. Cancelling loses no work: each batch
/// commits independently and the `orphaned_at` marks persist, so the next click
/// resumes exactly where the previous run stopped.
pub struct CleanupHandle {
    pub cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set while a cleanup is running. Serializes clicks so two concurrent
    /// clean_cache calls can't both drain and double-count — same busy-guard
    /// pattern as PruneHandle/BackupHandle.
    pub busy: std::sync::atomic::AtomicBool,
    /// Identity of the currently-running operation on the `task` event bus, if
    /// any — read by `stop_cleanup` to emit a `Cancelling` event. See tasks.rs.
    pub current_task: TaskSlot,
}

impl CleanupHandle {
    pub fn new() -> Self {
        Self {
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

    /// Lock-state probe that never materializes the key (unlike `get().is_err()`, which would
    /// copy the 32 key bytes into a temporary nothing zeroizes just to test a boolean).
    pub fn is_locked(&self) -> bool {
        self.0.lock().map(|g| g.is_none()).unwrap_or(true)
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
                id                     TEXT PRIMARY KEY,
                name                   TEXT NOT NULL,
                path                   TEXT NOT NULL,
                password_nonce         BLOB NOT NULL,
                password_ciphertext    BLOB NOT NULL,
                read_only              INTEGER NOT NULL DEFAULT 0,
                credentials_nonce      BLOB,
                credentials_ciphertext BLOB
            );
            CREATE TABLE IF NOT EXISTS backup_plans (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                repo_id         TEXT NOT NULL,
                paths_json      TEXT NOT NULL,
                tags_json       TEXT NOT NULL,
                excludes_json   TEXT NOT NULL,
                exclude_if_present_json TEXT,
                exclude_caches  INTEGER NOT NULL DEFAULT 0,
                retention_json  TEXT,
                limit_upload    INTEGER,
                limit_download  INTEGER,
                webhooks_json   TEXT
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
                raw_size         INTEGER,
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
        let _ = conn.execute_batch(
            "ALTER TABLE backup_plans ADD COLUMN exclude_if_present_json TEXT;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE backup_plans ADD COLUMN exclude_caches INTEGER NOT NULL DEFAULT 0;",
        );
        // Additive, nullable — per-plan webhook configs (URL + provider preset + stage
        // triggers) as a JSON array. NULL on pre-existing rows means "no webhooks" and
        // reads back as an empty Vec, identical to exclude_if_present_json. URLs are
        // stored plaintext — see docs/data.md.
        let _ = conn.execute_batch(
            "ALTER TABLE backup_plans ADD COLUMN webhooks_json TEXT;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE repositories ADD COLUMN read_only INTEGER NOT NULL DEFAULT 0;",
        );
        // Additive, nullable — an existing row's password_nonce/password_ciphertext are
        // never touched, and NULL here means "no stored credentials" (the ambient mode:
        // use restic's own credential chain). See CLAUDE.md's "Backend credentials".
        let _ = conn.execute_batch(
            "ALTER TABLE repositories ADD COLUMN credentials_nonce BLOB;",
        );
        let _ = conn.execute_batch(
            "ALTER TABLE repositories ADD COLUMN credentials_ciphertext BLOB;",
        );
        // Additive, nullable — on-disk stored size (post-dedup, post-compression) from a
        // second `restic stats --mode raw-data` call. An existing row's NULL here just means
        // "not yet refreshed since this field was added"; the frontend falls back to showing
        // only the restore-size figure it already had. See docs/restic.md.
        let _ = conn.execute_batch(
            "ALTER TABLE repo_stats_cache ADD COLUMN raw_size INTEGER;",
        );
        // Additive, nullable — the snapshot's logical size in bytes, taken from the
        // `summary.total_bytes_processed` field restic >=0.17 embeds in `snapshots --json`
        // output. NULL for a snapshot restic recorded without a summary (created by an older
        // restic, or by some `copy` operations); the frontend renders that as an em dash.
        // Snapshots are immutable, so this value never needs invalidation. See docs/restic.md.
        let _ = conn.execute_batch(
            "ALTER TABLE snapshots_cache ADD COLUMN size INTEGER;",
        );
        // Additive, nullable — unix-seconds timestamp set once a snapshot's browse-cache rows
        // are known to be orphaned (its id no longer appears in any repo's snapshots_cache).
        // NULL means "not orphaned". Used by mark_orphans/drain_orphans to delete
        // browse_cache_files in bounded batches instead of one unbounded transaction — see
        // clean_cache's doc comment and docs/data.md.
        let _ = conn.execute_batch(
            "ALTER TABLE indexed_snapshots ADD COLUMN orphaned_at INTEGER;",
        );
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
            .prepare("SELECT id, name, path, read_only FROM repositories ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let repos = stmt
            .query_map([], |row| {
                Ok(Repository {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    read_only: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(repos)
    }

    /// Repo display name only — deliberately separate from `get_full_repo`, whose `path` can
    /// carry inline REST userinfo credentials (`rest:https://user:PASS@host/repo`) and must
    /// never be used in a notification body. See `notify::notify`'s "Backup started" call site.
    pub fn get_repo_name(&self, repo_id: &str) -> Result<String, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT name FROM repositories WHERE id = ?1",
            params![repo_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| e.to_string())
    }

    pub fn get_full_repo(&self, repo_id: &str, key: &[u8; 32]) -> Result<FullRepository, String> {
        let (path, nonce, ciphertext, read_only, cred_nonce, cred_ciphertext) = {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT path, password_nonce, password_ciphertext, read_only,
                        credentials_nonce, credentials_ciphertext
                 FROM repositories WHERE id = ?1",
                params![repo_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)? != 0,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                },
            )
            .map_err(|e| format!("Repository not found: {e}"))?
        };
        let (password, credentials) = decode_secrets(
            key,
            &nonce,
            &ciphertext,
            cred_nonce.as_deref(),
            cred_ciphertext.as_deref(),
        )?;
        Ok(FullRepository { path, password, read_only, credentials })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_repo(
        &self,
        id: &str,
        name: &str,
        path: &str,
        nonce: &[u8],
        ciphertext: &[u8],
        read_only: bool,
        cred_nonce: Option<&[u8]>,
        cred_ciphertext: Option<&[u8]>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO repositories
             (id, name, path, password_nonce, password_ciphertext, read_only,
              credentials_nonce, credentials_ciphertext)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, name, path, nonce, ciphertext, read_only, cred_nonce, cred_ciphertext],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Removes the repo row plus the small, PK-indexed cache rows keyed directly by
    /// `repo_id` (`browse_cache_status`, `snapshots_cache`, `repo_stats_cache`) — all
    /// bounded by this repo's snapshot count, so this stays fast even for a
    /// long-lived repo. Deliberately does **not** cascade into `browse_cache_files`/
    /// `indexed_snapshots` (keyed by `snapshot_id`, not `repo_id`): those can hold one
    /// row per file across every indexed snapshot, so for a repo with a lot of
    /// indexing history that delete can be enormous and slow — this used to run
    /// inline here, making "remove repository" hang for minutes. Deleting
    /// `snapshots_cache` above is what turns those rows into orphans (no remaining
    /// `snapshots_cache` entry references their `snapshot_id`), which orphan cleanup
    /// already exists to sweep up — both the automatic ~5-minute tick in `cache_warmer.rs`
    /// and the "Clean Orphaned Data" button (`clean_cache`) — see `mark_orphans`'s doc
    /// comment for the matching orphan definition.
    pub fn remove_repo(&self, repo_id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM repositories WHERE id = ?1", params![repo_id])
            .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM browse_cache_status WHERE repo_id = ?1",
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

    pub fn set_repo_read_only(&self, repo_id: &str, read_only: bool) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE repositories SET read_only = ?1 WHERE id = ?2",
            params![read_only, repo_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Returns each stored credential's key and value. Same threat model as
    /// `get_repo_password` — the password already crosses IPC as plaintext for the
    /// edit modal, and credentials use the identical pipeline (AES-GCM at rest under
    /// the master key, decrypted on demand, masked `type="password"` input on the
    /// frontend). Returning values here lets the edit modal (a) populate rows with
    /// the actual secret so Test Connection reflects reality, and (b) save a no-op
    /// edit without forcing the user to retype every value.
    pub fn get_repo_credentials(
        &self,
        repo_id: &str,
        key: &[u8; 32],
    ) -> Result<Vec<(String, String)>, String> {
        let full = self.get_full_repo(repo_id, key)?;
        Ok(full.credentials.iter().map(|c| (c.key.clone(), c.value.clone())).collect())
    }

    /// Read-modify-write update of a repo's password and/or credentials, both in one
    /// transaction so two sequential edits (e.g. from RepositoriesPage's edit modal,
    /// which dirty-checks password and credentials independently) can never lose one
    /// or the other. `password`/`credentials` of `None` leaves that field unchanged;
    /// `Some` replaces it entirely (an empty `Vec` clears stored credentials back to
    /// ambient mode).
    pub fn update_repo_secrets(
        &self,
        repo_id: &str,
        key: &[u8; 32],
        password: Option<String>,
        credentials: Option<Vec<Credential>>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let (cur_pw_nonce, cur_pw_ct, cur_cred_nonce, cur_cred_ct): RepoSecretRow = tx
            .query_row(
                "SELECT password_nonce, password_ciphertext, credentials_nonce, credentials_ciphertext
                 FROM repositories WHERE id = ?1",
                params![repo_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| format!("Repository not found: {e}"))?;

        let (new_pw_nonce, new_pw_ct) = if let Some(pw) = password {
            let mut bytes = pw.into_bytes();
            let enc = crypto::encrypt(key, &bytes)?;
            bytes.zeroize();
            enc
        } else {
            (cur_pw_nonce, cur_pw_ct)
        };

        let (new_cred_nonce, new_cred_ct) = if let Some(creds) = credentials {
            encode_credentials(key, &creds)?
        } else {
            (cur_cred_nonce, cur_cred_ct)
        };

        tx.execute(
            "UPDATE repositories
             SET password_nonce = ?1, password_ciphertext = ?2,
                 credentials_nonce = ?3, credentials_ciphertext = ?4
             WHERE id = ?5",
            params![new_pw_nonce, new_pw_ct, new_cred_nonce, new_cred_ct, repo_id],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Atomically rotate the master key: re-encrypt every repo password and every
    /// repo's stored backend credentials with the new key, rewrite the verification
    /// row, and finally re-read every row's secrets back through the same
    /// `decode_secrets` path `get_full_repo` uses (with the *new* key) before
    /// committing. Either all of it commits or none of it does — so a crash, or a
    /// future encrypted field someone forgets to wire into this loop, can't leave
    /// some secrets on the new key while others (or the verification row) still
    /// expect the old one. The verification pass is what makes that guarantee hold
    /// for fields added after this one: it doesn't know what "credentials" are, it
    /// just proves everything `get_full_repo` would need to decrypt actually does.
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

        // Re-encrypt every repo's password and stored credentials with the new key.
        // If any row fails to decrypt, the `?` returns and the transaction is rolled
        // back on drop.
        let rows: Vec<RepoSecretRowWithId> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, password_nonce, password_ciphertext,
                            credentials_nonce, credentials_ciphertext
                     FROM repositories",
                )
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        for (id, pw_nonce, pw_ct, cred_nonce, cred_ct) in rows {
            let mut pw = crypto::decrypt(old_key, &pw_nonce, &pw_ct)?;
            let (new_pw_nonce, new_pw_ct) = crypto::encrypt(new_key, &pw)?;
            pw.zeroize();

            let (new_cred_nonce, new_cred_ct) = match (cred_nonce, cred_ct) {
                (Some(cn), Some(cc)) => {
                    let mut plaintext = crypto::decrypt(old_key, &cn, &cc)?;
                    let re_encrypted = crypto::encrypt(new_key, &plaintext)?;
                    plaintext.zeroize();
                    (Some(re_encrypted.0), Some(re_encrypted.1))
                }
                _ => (None, None),
            };

            tx.execute(
                "UPDATE repositories
                 SET password_nonce = ?1, password_ciphertext = ?2,
                     credentials_nonce = ?3, credentials_ciphertext = ?4
                 WHERE id = ?5",
                params![new_pw_nonce, new_pw_ct, new_cred_nonce, new_cred_ct, id],
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

        // Verification pass: prove every row's secrets actually decrypt under the new
        // key before committing anything. This is the guard against a future
        // encrypted field being added to `repositories` without a matching re-encrypt
        // step above — such a row would still be under the old key and would fail
        // here, rolling the whole rotation back, rather than silently orphaning it.
        let verify_rows: Vec<RepoSecretRow> = {
            let mut stmt = tx
                .prepare(
                    "SELECT password_nonce, password_ciphertext,
                            credentials_nonce, credentials_ciphertext
                     FROM repositories",
                )
                .map_err(|e| e.to_string())?;
            let collected = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        for (pw_nonce, pw_ct, cred_nonce, cred_ct) in verify_rows {
            decode_secrets(
                new_key,
                &pw_nonce,
                &pw_ct,
                cred_nonce.as_deref(),
                cred_ct.as_deref(),
            )?;
        }

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── backup plans ────────────────────────────────────────────────────────

    pub fn list_backup_plans(&self) -> Result<Vec<BackupPlan>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json FROM backup_plans ORDER BY name COLLATE NOCASE")
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
                    row.get::<_, bool>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<u32>>(9)?,
                    row.get::<_, Option<u32>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        plans
            .into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json)| {
                Ok(BackupPlan {
                    id,
                    name,
                    repo_id,
                    paths: serde_json::from_str(&paths_json).map_err(|e| e.to_string())?,
                    tags: serde_json::from_str(&tags_json).map_err(|e| e.to_string())?,
                    excludes: serde_json::from_str(&excludes_json).map_err(|e| e.to_string())?,
                    exclude_if_present: exclude_if_present_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?
                        .unwrap_or_default(),
                    exclude_caches,
                    retention: retention_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?,
                    limit_upload,
                    limit_download,
                    webhooks: webhooks_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn save_backup_plan(&self, plan: &BackupPlan) -> Result<(), String> {
        let paths_json = serde_json::to_string(&plan.paths).map_err(|e| e.to_string())?;
        let tags_json = serde_json::to_string(&plan.tags).map_err(|e| e.to_string())?;
        let excludes_json = serde_json::to_string(&plan.excludes).map_err(|e| e.to_string())?;
        let exclude_if_present_json =
            serde_json::to_string(&plan.exclude_if_present).map_err(|e| e.to_string())?;
        let webhooks_json =
            serde_json::to_string(&plan.webhooks).map_err(|e| e.to_string())?;
        let retention_json = plan
            .retention
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e: serde_json::Error| e.to_string())?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO backup_plans
             (id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                plan.id,
                plan.name,
                plan.repo_id,
                paths_json,
                tags_json,
                excludes_json,
                exclude_if_present_json,
                plan.exclude_caches,
                retention_json,
                plan.limit_upload,
                plan.limit_download,
                webhooks_json,
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

    /// Lightweight single-plan lookup for the webhook fire path — returns the plan's
    /// name (for payload text) and webhook list without materializing a full
    /// `BackupPlan`. `Ok(None)` = plan id doesn't exist (e.g. deleted mid-backup) →
    /// caller fires nothing.
    pub fn get_plan_webhooks(
        &self,
        plan_id: &str,
    ) -> Result<Option<(String, Vec<PlanWebhook>)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT name, webhooks_json FROM backup_plans WHERE id = ?1",
            params![plan_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        ) {
            Ok((name, webhooks_json)) => Ok(Some((
                name,
                webhooks_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e: serde_json::Error| e.to_string())?
                    .unwrap_or_default(),
            ))),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
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

    /// Reads several `app_settings` keys under a single mutex acquisition/query, rather than one
    /// `get_setting` (one lock + one query each) per key — used by `notify::load`, which would
    /// otherwise take the shared `AppDb` mutex 5 times per notification (up to 10 times per
    /// backup). Keys absent from the table are simply absent from the returned map; callers
    /// apply their own per-key defaults.
    pub fn get_settings(&self, keys: &[&str]) -> Result<std::collections::HashMap<String, String>, String> {
        // An empty slice would otherwise build `WHERE key IN ()` — a SQL syntax error.
        if keys.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let placeholders = keys.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT key, value FROM app_settings WHERE key IN ({placeholders})");
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(keys.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
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

    /// Writes several `app_settings` keys atomically in one transaction — used by
    /// `notify::save`, which writes five keys per call. Without this, two overlapping
    /// `set_notification_settings` invocations (Tauri offloads sync commands to its own thread
    /// pool with no ordering guarantee between separate invoke calls) could interleave their
    /// five individual `set_setting` writes, leaving `app_settings` holding a mix of fields from
    /// two different saves rather than either call's value cleanly winning.
    pub fn set_settings(&self, entries: &[(&str, &str)]) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for (key, value) in entries {
            tx.execute(
                "INSERT OR REPLACE INTO app_settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
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
        // Upsert, not `INSERT OR IGNORE`: if this snapshot's row was already marked
        // `orphaned_at` by a `mark_orphans` scan (e.g. it briefly dropped out of
        // `snapshots_cache` and came back while a `drain_orphans` batch loop was still
        // running), indexing it here is proof it is live again and the mark must be
        // cleared *before* any `browse_cache_files` rows are written below — otherwise a
        // concurrent drain would delete file rows out from under this call, and since the
        // resulting `browse_cache_status` reads "complete", nothing would ever retry it.
        // See docs/decisions.md.
        tx.execute(
            "INSERT INTO indexed_snapshots (snapshot_id) VALUES (?1)
             ON CONFLICT(snapshot_id) DO UPDATE SET orphaned_at = NULL",
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
        match conn
            .prepare_cached("SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1")
            .map_err(|e| e.to_string())?
            .query_row(params![snapshot_id], |row| row.get(0))
        {
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
            // Partially indexed: return whatever was cached for this directory. This
            // also serves the interactive-browse cache (rows written by `set` with no
            // status row at all) and, after a Clear Index of a snapshot whose shared
            // rows evict kept for another repo, that other repo's rows — deliberate:
            // see docs/decisions.md ("browse serves any cached rows for the snapshot;
            // search and the index badge reflect this repo's own index state").
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

    /// Clears one snapshot's index for the separate "clear index" action: always this repo's
    /// `browse_cache_status` row, plus the shared `browse_cache_files` rows and
    /// `indexed_snapshots` mapping **only when no other repo has a readable index of that
    /// snapshot** — i.e. no other repo's 'complete' status row. The guard deliberately keys
    /// on *complete* status rows, not `snapshots_cache` listings, and each difference is the
    /// point:
    /// - Status rows, not listings: status rows are the *consumers* of the shared rows (a
    ///   repo whose index is 'complete' browses and searches them directly), while a repo
    ///   that merely lists the snapshot does not. Keying on listings stranded and over-kept
    ///   at once: after B forgets its copy (`remove_snapshot_from_cache` deletes only the
    ///   listing — B's 'complete' status survives), a Clear in A would delete the shared
    ///   rows under B's surviving status — B then browses a permanently empty tree nothing
    ///   retries ('complete' is excluded from `get_next_unindexed_snapshot`) and no sweep
    ///   removes (the snapshot is still live in A); conversely a B that only *lists* the
    ///   snapshot (never indexed it) would keep the rows alive forever for no reader.
    /// - 'complete' rows only: a repo whose run is merely 'in_progress' or left 'pending'
    ///   has no readable index, and deleting under it is safe by construction — an
    ///   in-flight run's terminal write is gated on a *live* mapping
    ///   (`set_browse_status_complete_if_live`), so a Clear that lands anywhere during that
    ///   run — including after its last chunk, or during a zero-entry run that writes no
    ///   chunks at all — makes the terminal write fail closed instead of resurrecting
    ///   'complete' over zero rows; the run then evicts its own writes and retries
    ///   ('pending' stays pickable). A 'pending' row pins nothing but its own retry.
    ///   Counting those statuses would recreate the listings-keying under-clear: shared
    ///   rows kept alive forever by a repo that will never read them. Sequential clears
    ///   in each repo fully reclaim. This is safe because `mark_orphans`' status delete
    ///   is global by snapshot_id — by the time drain deletes rows, every status row for
    ///   that snapshot is already gone in the same marking transaction, so a surviving
    ///   status row always implies the snapshot is still listed and the rows are live
    ///   cache. (`mark_orphans` itself still keys on listings, correctly: liveness is
    ///   its question.)
    ///
    /// See `evict_keys_the_sharing_guard_on_status_rows_not_listings`.
    ///
    /// The sharing probe runs once in Rust and gates both deletes (equivalent to a NOT EXISTS
    /// per statement, without evaluating it — with an unsargable `repo_id <> ?` — twice).
    /// File rows delete before the mapping, per `drain_orphans`' ordering rule. All deletes
    /// in **one transaction** — not for in-process interleaving (the single connection's
    /// mutex already made the old three-statement sequence non-interleavable), but for
    /// crash/error atomicity: a failure after the file-rows delete would otherwise leave
    /// 'complete' + zero rows + a live mapping — the stuck empty-tree state, reachable by a
    /// crash rather than a race.
    pub fn evict(&self, repo_id: &str, snapshot_id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM browse_cache_status WHERE repo_id = ?1 AND snapshot_id = ?2",
            params![repo_id, snapshot_id],
        )
        .map_err(|e| e.to_string())?;
        let shared: bool = tx
            .prepare_cached(
                "SELECT EXISTS(
                     SELECT 1 FROM browse_cache_status
                     WHERE snapshot_id = ?2 AND repo_id <> ?1 AND status = 'complete'
                 )",
            )
            .map_err(|e| e.to_string())?
            .query_row(params![repo_id, snapshot_id], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if !shared {
            // Neither DELETE references repo_id — `browse_cache_files`/
            // `indexed_snapshots` have no repo column — so both bind snapshot_id alone.
            tx.execute(
                "DELETE FROM browse_cache_files
                 WHERE snap = (SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1)",
                params![snapshot_id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM indexed_snapshots WHERE snapshot_id = ?1",
                params![snapshot_id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Removes just this one snapshot's `snapshots_cache` row after a successful `forget` —
    /// keeps the repo's cached snapshot list accurate immediately, without an unbounded
    /// `browse_cache_files` delete (`evict`'s job, for the separate "clear index" action) or
    /// wiping the rest of the repo's cache (the old `evict_snapshots`-on-delete pattern,
    /// removed — see docs/decisions.md). Deliberately touches nothing else:
    /// `indexed_snapshots`/`browse_cache_files`/`browse_cache_status` for this snapshot are
    /// left as-is, discovered and swept by the next orphan-cleanup run — the automatic
    /// ~5-minute tick in `cache_warmer.rs` or the "Clean Orphaned Data" button — same as
    /// every other orphan source (retention, repo removal).
    pub fn remove_snapshot_from_cache(&self, repo_id: &str, snapshot_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM snapshots_cache WHERE repo_id = ?1 AND snapshot_id = ?2",
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

    /// Plain upsert of a status row. Every production write now goes through a gated
    /// variant — `set_browse_status_if_listed` for an index run's 'in_progress' entry
    /// write, `set_browse_status_if_present` for its terminal/failure writes — so this
    /// survives as the test fixture's seeding helper.
    #[cfg(test)]
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

    /// `set_browse_status`, but a strict no-op when the (repo_id, snapshot_id) status row does
    /// not exist. The terminal writes of an index run — the trailing 'complete', and the
    /// failure-path 'pending' — may modify an existing row but must never *resurrect* one that
    /// `evict` ("Clear Index") or `mark_orphans` deleted mid-run: resurrecting 'complete'
    /// strands an empty-tree snapshot nothing retries (`get_next_unindexed_snapshot` picks only
    /// NULL/'pending', and `mark_orphans` only sweeps statuses for snapshots absent from
    /// `snapshots_cache`), and resurrecting 'pending' re-arms the auto-indexer against the
    /// user's explicit Clear — or, post-drain, flips `has_cleanup_work` true solely to delete
    /// the row this code just wrote. In every normal flow the row exists ('in_progress' was
    /// written before the run), so the gate passes and this behaves exactly like
    /// `set_browse_status`. Implemented as a plain `UPDATE` — it can never insert — and returns
    /// the rows affected: **0 means the status row vanished mid-run** (`evict`/"Clear Index",
    /// `mark_orphans`, `clear_cache`), which `run_full_index` treats as an abort (see its
    /// doc comment). Single autocommit statement — atomic, no window.
    pub fn set_browse_status_if_present(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        status: &str,
    ) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE browse_cache_status SET status = ?3, cached_at = ?4
             WHERE repo_id = ?1 AND snapshot_id = ?2",
            params![repo_id, snapshot_id, status, timestamp()],
        )
        .map_err(|e| e.to_string())
    }

    /// `set_browse_status_if_present`, but for the success-path 'complete' write specifically —
    /// gated on a *live mapping*, not just a present status row. `set_browse_status_if_present`
    /// alone isn't enough here: this repo's own status row can still be present while the shared
    /// rows this run just wrote are gone, if a **different** repo's Clear Index (`evict`) ran
    /// between this run's last `insert_browse_files` chunk and this call. `evict` has no
    /// per-snapshot lock to exclude that window (`clear_snapshot_index` is sync, takes no
    /// `IndexHandle::gate`), and the zero-entry case runs no chunk liveness check at all — so
    /// without this, the run's own status row would flip to 'complete' with zero
    /// `browse_cache_files` rows and no `indexed_snapshots` mapping, a state nothing retries
    /// (`get_next_unindexed_snapshot` excludes 'complete') and nothing sweeps (the snapshot is
    /// still listed, so `mark_orphans` leaves it alone) — a permanently empty browse/search tree.
    /// `snap` is the id `insert_browse_files` interned and wrote every chunk's rows against;
    /// requiring `indexed_snapshots` to still map `snapshot_id` to that same `snap` (not just to
    /// *a* row) matters because id is INTEGER PRIMARY KEY without AUTOINCREMENT — SQLite recycles
    /// a freed rowid, so a bare "does a mapping exist" check could pass against a completely
    /// different, freshly re-interned run. 0 rows affected means either this repo's status row
    /// vanished (same-repo Clear, `mark_orphans`, `clear_cache`) or the mapping this run wrote
    /// against is dead (another repo's Clear, or a `drain_orphans` retire) — `run_full_index`
    /// treats both the same way: evict this run's own writes and report failure. Single
    /// autocommit statement — atomic, no window.
    pub fn set_browse_status_complete_if_live(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        snap: i64,
    ) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE browse_cache_status SET status = 'complete', cached_at = ?4
             WHERE repo_id = ?1 AND snapshot_id = ?2
               AND EXISTS(
                   SELECT 1 FROM indexed_snapshots WHERE id = ?3 AND snapshot_id = ?2
               )",
            params![repo_id, snapshot_id, snap, timestamp()],
        )
        .map_err(|e| e.to_string())
    }

    /// `set_browse_status`, but a no-op when this repo no longer lists the snapshot: an
    /// upsert gated on a `snapshots_cache` row for (repo_id, snapshot_id). The 'in_progress'
    /// *entry* writes of an index run go through here so a run can never (re)create a status
    /// row for a snapshot that was forgotten — by an external restic, or via
    /// `remove_snapshot_from_cache` — between the run being queued and starting: a plain
    /// `set_browse_status` there would leave a status row for an unlisted snapshot, flipping
    /// `has_cleanup_work` true until the next mark run deleted the row the run just wrote.
    /// Returns the rows affected: **0 means the snapshot is no longer listed — skip the run
    /// entirely** (there is nothing to index against and no status row to report through).
    /// First-time indexing is unaffected: a listed snapshot with no status row gets one
    /// (this is an upsert, unlike `set_browse_status_if_present`'s UPDATE).
    pub fn set_browse_status_if_listed(
        &self,
        repo_id: &str,
        snapshot_id: &str,
        status: &str,
    ) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO browse_cache_status (repo_id, snapshot_id, status, cached_at)
             SELECT ?1, ?2, ?3, ?4
             WHERE EXISTS(
                 SELECT 1 FROM snapshots_cache WHERE repo_id = ?1 AND snapshot_id = ?2
             )",
            params![repo_id, snapshot_id, status, timestamp()],
        )
        .map_err(|e| e.to_string())
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
    /// Inserts in chunks of 500 to avoid holding the mutex for excessive time. Returns the
    /// interned `snap` id every chunk was written against, so the caller's terminal write can
    /// gate on that exact mapping still being live — see
    /// `set_browse_status_complete_if_live`'s doc comment.
    pub fn insert_browse_files(
        &self,
        snapshot_id: &str,
        entries: &[FileEntry],
    ) -> Result<i64, String> {
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
            self.insert_browse_files_chunk(snap, snapshot_id, chunk)?;
        }
        Ok(snap)
    }

    /// Writes one chunk of file rows against an already-interned `snap` id. Split out of
    /// `insert_browse_files` so the mapping-retired-mid-run case below is unit-testable
    /// without real concurrency.
    fn insert_browse_files_chunk(
        &self,
        snap: i64,
        snapshot_id: &str,
        chunk: &[FileEntry],
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        // The interned mapping this chunk writes against must still exist. A concurrent
        // drain_orphans can retire it mid-run — retire fires only at zero file rows, i.e.
        // after deleting every row this run already wrote — or evict can remove it while the
        // snapshot is still live. Writing against a dead id would strand rows invisible to
        // every query *and* every sweep, since all reads join through indexed_snapshots.
        // Keyed on (id, snapshot_id), not id alone: id is INTEGER PRIMARY KEY without
        // AUTOINCREMENT, so SQLite recycles a freed max rowid — id alone could validate a
        // *different* snapshot's freshly-interned mapping handed the recycled id (reachable
        // mid-run via the ungated `set` browse path; bulk index runs can't, IndexHandle::gate
        // serializes them app-wide).
        //
        // The check shares this chunk's transaction, and the single AppDb connection
        // serializes transactions, so it is atomic against both: either they ran first
        // (mapping gone → abort now, with this run's earlier rows already deleted) or they
        // run after and see this chunk's rows. One PK lookup per 500-row chunk — strictly
        // cheaper than the per-chunk INSERT OR IGNORE + SELECT intern this loop once
        // carried before that was optimized away. See docs/decisions.md.
        if Self::snap_id_of(&tx, snapshot_id)? != Some(snap) {
            return Err(format!(
                "index aborted: snapshot {snapshot_id} was removed while being indexed \
                 (its cached index rows were cleaned up)"
            ));
        }
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
                "SELECT snapshot_id, short_id, time, hostname, username, paths, tags, size
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
                    size: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }

    /// Full sync of a repo's cached snapshot list against a fresh `restic snapshots --json`
    /// listing — diff-based, not a blind delete-all-then-insert-all: only ids that actually
    /// dropped out of the new listing are deleted, and every id in the new listing is
    /// upserted (never skipped), so this stays exactly as correct as a full replace for
    /// picking up a tag/metadata change on an id that neither added nor dropped, while
    /// avoiding rewriting every row in the repo's history whenever only one snapshot
    /// changed. Deleting only confirmed-dropped ids (never "everything for this repo") is
    /// also what keeps a caller from ever manufacturing an ambiguous empty cache by feeding
    /// this an empty/partial list to represent "unknown" — the caller must always pass a
    /// real, successfully-fetched listing; see docs/decisions.md.
    pub fn set_snapshots(&self, repo_id: &str, json: &str) -> Result<(), String> {
        let rows = parse_snapshot_rows(json)?;
        let now = timestamp();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let old_ids: std::collections::HashSet<String> = {
            let mut stmt = tx
                .prepare_cached("SELECT snapshot_id FROM snapshots_cache WHERE repo_id = ?1")
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(params![repo_id], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            mapped
        };
        let new_ids: std::collections::HashSet<&str> =
            rows.iter().map(|s| s.id.as_str()).collect();

        let dropped: Vec<&str> = old_ids
            .iter()
            .filter(|id| !new_ids.contains(id.as_str()))
            .map(|id| id.as_str())
            .collect();
        if !dropped.is_empty() {
            // Purely anonymous `?` placeholders, bound positionally — same convention as
            // the other dynamic IN-lists in this file (e.g. get_settings above).
            let placeholders = dropped.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "DELETE FROM snapshots_cache WHERE repo_id = ? AND snapshot_id IN ({placeholders})"
            );
            let mut p: Vec<&dyn rusqlite::ToSql> = vec![&repo_id];
            p.extend(dropped.iter().map(|id| id as &dyn rusqlite::ToSql));
            tx.execute(&sql, p.as_slice()).map_err(|e| e.to_string())?;
        }

        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO snapshots_cache
                     (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, size, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT (repo_id, snapshot_id) DO UPDATE SET
                       short_id = excluded.short_id,
                       time = excluded.time,
                       hostname = excluded.hostname,
                       username = excluded.username,
                       paths = excluded.paths,
                       tags = excluded.tags,
                       size = excluded.size,
                       cached_at = excluded.cached_at",
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
                    s.size,
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
                     (repo_id, snapshot_id, short_id, time, hostname, username, paths, tags, size, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                    s.size,
                    now
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    // ── repo stats cache ─────────────────────────────────────────────────────

    /// `cached_at` on the returned `ResticStats` is a Unix-seconds timestamp — surfaced to
    /// the frontend as a "Refreshed …" label on RepositoriesPage now that stats are
    /// manual-refresh-only (see `set_stats`). `raw_size` is `None` for a row cached before
    /// that field existed, or before its most recent refresh's raw-data call last failed.
    pub fn get_stats(&self, repo_id: &str) -> Result<Option<ResticStats>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        match conn.query_row(
            "SELECT total_size, total_file_count, snapshots_count, raw_size, cached_at
             FROM repo_stats_cache WHERE repo_id = ?1",
            params![repo_id],
            |row| {
                Ok(ResticStats {
                    total_size: row.get::<_, i64>(0)? as u64,
                    total_file_count: row.get::<_, i64>(1)? as u64,
                    snapshots_count: row.get::<_, i64>(2)? as u64,
                    raw_size: row.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                    cached_at: Some(row.get::<_, i64>(4)?),
                })
            },
        ) {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Writes fresh stats and returns the `cached_at` timestamp it wrote, so the caller
    /// (`fetch_and_cache_stats` in repo.rs) can hand it straight back to the frontend
    /// without a re-read. `stats.cached_at` itself is ignored — this always stamps `now`.
    pub fn set_stats(&self, repo_id: &str, stats: &ResticStats) -> Result<i64, String> {
        let now = timestamp();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO repo_stats_cache
             (repo_id, total_size, total_file_count, snapshots_count, raw_size, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                repo_id,
                stats.total_size as i64,
                stats.total_file_count as i64,
                stats.snapshots_count as i64,
                stats.raw_size.map(|v| v as i64),
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

        rows.into_iter().map(row_to_schedule).collect()
    }

    /// Looks up a single schedule by id, `Ok(None)` if it doesn't exist. Used by
    /// `save_schedule` (the command) to preserve `last_run_at`/`created_at` across an edit —
    /// see its own doc comment for why those two fields must survive a save that isn't a
    /// fresh creation.
    pub fn get_schedule(&self, id: &str) -> Result<Option<Schedule>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let row = conn
            .query_row(
                "SELECT id, name, plan_ids_json, cron_expr, enabled, last_run_at, next_run_at, created_at
                 FROM schedules WHERE id = ?1",
                params![id],
                |row| {
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
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;

        row.map(row_to_schedule).transpose()
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

    /// Updates only `next_run_at` for a schedule, without touching `last_run_at`
    /// or `enabled`. Used by `toggle_schedule` when enabling, to recompute the
    /// next fire time so a re-enabled schedule doesn't immediately fire as
    /// "missed" (its `next_run_at` may be stale from however long it was
    /// disabled).
    pub fn set_schedule_next_run(&self, id: &str, next_run_at: Option<i64>) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE schedules SET next_run_at = ?1 WHERE id = ?2",
            params![next_run_at, id],
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
            "SELECT id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json
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
                    row.get::<_, bool>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<u32>>(9)?,
                    row.get::<_, Option<u32>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|(id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json)| {
                Ok(BackupPlan {
                    id,
                    name,
                    repo_id,
                    paths: serde_json::from_str(&paths_json).map_err(|e: serde_json::Error| e.to_string())?,
                    tags: serde_json::from_str(&tags_json).map_err(|e: serde_json::Error| e.to_string())?,
                    excludes: serde_json::from_str(&excludes_json).map_err(|e: serde_json::Error| e.to_string())?,
                    exclude_if_present: exclude_if_present_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?
                        .unwrap_or_default(),
                    exclude_caches,
                    retention: retention_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?,
                    limit_upload,
                    limit_download,
                    webhooks: webhooks_json
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|e: serde_json::Error| e.to_string())?
                        .unwrap_or_default(),
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

    /// Wipes every cache table (all rebuilt on next use — see docs/data.md). Deliberately
    /// does **not** `VACUUM`: each `DELETE FROM table;` here has no `WHERE` clause, so
    /// SQLite's truncate optimization deallocates the whole table in one step regardless
    /// of row count — cheap and fast no matter how large the cache was. `VACUUM` is a
    /// different cost entirely: it rewrites every live page in the database, scaling with
    /// total DB size rather than rows deleted, and holds the same single `AppDb` connection
    /// mutex for however long that takes — on a multi-GB database, long enough to be the
    /// same class of freeze `clean_cache`'s old unbounded transaction caused (see its own
    /// doc comment). `VACUUM` isn't batchable the way a row-scoped delete is either (it
    /// would need `auto_vacuum = INCREMENTAL`, a real schema change, not just chunking).
    /// `compress_database` ("Compress Database") already exists as the dedicated place for
    /// that cost — reclaiming space is deliberately its job alone, not an implicit side
    /// effect of clearing data here. Run it after this if the freed space needs to be
    /// returned to the OS.
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
        Ok(self.checkpoint_and_size(&conn))
    }

    /// Deletes the small repo-keyed orphan rows (`snapshots_cache`/`repo_stats_cache`
    /// whose `repo_id` no longer exists — e.g. a removed repo), then, using that
    /// now-up-to-date `snapshots_cache`, marks every `indexed_snapshots` row no longer
    /// referenced by *any* repo (stamping `orphaned_at`) and deletes every
    /// `browse_cache_status` row in the same position. Does **not** touch
    /// `browse_cache_files`; `drain_orphans` does that, in bounded batches. Returns the
    /// number of rows actually deleted this call (repo-keyed + status rows) — marking
    /// alone isn't counted, since nothing is removed until `drain_orphans` retires it.
    ///
    /// The repo-keyed sweep must run **before** the orphan check below, in the same
    /// transaction: removing a dead repo's `snapshots_cache` row is what turns its
    /// snapshot's `browse_cache_status`/`indexed_snapshots` rows into orphans in the
    /// first place. Reversing the order would leave last call's dead-repo rows
    /// invisible to this call's orphan check for one extra call.
    ///
    /// `snapshots_cache` is global, not per-repo, so "referenced" means "referenced by
    /// *any* repo" — this preserves clean_cache's original semantics where two repos
    /// sharing a snapshot id (no `repo_id` column on `browse_cache_files`/
    /// `indexed_snapshots`) keep each other's rows alive. See
    /// `remove_repo_keeps_file_rows_shared_with_another_repo`.
    ///
    /// Idempotent: re-running only advances `indexed_snapshots` rows currently `NULL`,
    /// and un-marks anything that reappeared in `snapshots_cache` since being marked
    /// (e.g. a repo re-added between runs) — its `browse_cache_status` row is already
    /// gone by then, so it simply re-indexes, which is correct since its file rows may
    /// already be partially drained.
    pub fn mark_orphans(&self) -> Result<u64, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let now = timestamp();
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

        tx.execute(
            "UPDATE indexed_snapshots
             SET orphaned_at = ?1
             WHERE orphaned_at IS NULL
               AND snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)",
            params![now],
        )
        .map_err(|e| e.to_string())?;

        removed += tx
            .execute(
                "DELETE FROM browse_cache_status
                 WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        // Un-mark anything that reappeared in snapshots_cache since being marked.
        tx.execute(
            "UPDATE indexed_snapshots
             SET orphaned_at = NULL
             WHERE orphaned_at IS NOT NULL
               AND snapshot_id IN (SELECT snapshot_id FROM snapshots_cache)",
            [],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }

    /// Cheap read-only probe for "would `mark_orphans`/`drain_orphans` find anything to do
    /// right now?" — no transaction, just a boolean `EXISTS` chain. Used to keep the
    /// automatic cleanup tick (`cache_warmer.rs`) from paying for a write transaction on
    /// every 5-minute tick when there is nothing to clean, which is the overwhelmingly
    /// common case. The "Clean Orphaned Data" button does not call this — a manual click
    /// always runs for real, since the user explicitly asked for work and expects a task
    /// row plus a refreshed DB size regardless of whether anything was found.
    ///
    /// The five clauses mirror `mark_orphans`'s four statements exactly, plus a fifth for
    /// "already marked and still awaiting `drain_orphans`" (covers a run interrupted before
    /// it finished draining). This duplication is the one real maintenance hazard here:
    /// **any new statement added to `mark_orphans` needs a matching clause added here**,
    /// or this probe can go stale and start saying "nothing to do" when there is. Pinned by
    /// `has_cleanup_work_agrees_with_mark_and_drain` below rather than left to this comment
    /// alone.
    pub fn has_cleanup_work(&self) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT
               EXISTS(SELECT 1 FROM snapshots_cache
                        WHERE repo_id NOT IN (SELECT id FROM repositories))
               OR EXISTS(SELECT 1 FROM repo_stats_cache
                        WHERE repo_id NOT IN (SELECT id FROM repositories))
               OR EXISTS(SELECT 1 FROM indexed_snapshots
                        WHERE orphaned_at IS NULL
                          AND snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache))
               OR EXISTS(SELECT 1 FROM browse_cache_status
                        WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache))
               OR EXISTS(SELECT 1 FROM indexed_snapshots WHERE orphaned_at IS NOT NULL)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    /// Number of `browse_cache_files` rows still waiting on `drain_orphans` — i.e. rows
    /// belonging to a snapshot `mark_orphans` has already marked. Used to give a
    /// progress bar a denominator; a run's actual `rows_deleted` total can slightly
    /// exceed this by the end (it also counts the retired `indexed_snapshots` rows and
    /// `mark_orphans`'s own repo-keyed/status deletions) — callers should clamp for
    /// display rather than try to make the two sums match exactly.
    pub fn pending_orphan_row_count(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT COUNT(*) FROM browse_cache_files
             WHERE snap IN (SELECT id FROM indexed_snapshots WHERE orphaned_at IS NOT NULL)",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
    }

    /// Deletes up to `max_rows` orphaned `browse_cache_files` rows (marked by a prior
    /// `mark_orphans` call) in one short transaction, and retires any `indexed_snapshots`
    /// row left with zero file rows. Call repeatedly (checking `more_remaining`) to
    /// drain a large backlog without holding the single `AppDb` connection mutex for
    /// more than one batch at a time — see `clean_cache`'s doc comment for why that
    /// matters. Does not repeat `mark_orphans`'s repo-keyed sweep; call that first.
    pub fn drain_orphans(&self, max_rows: usize) -> Result<DrainBatch, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut removed = 0u64;

        // Bounded file-row delete. SQLite only accepts LIMIT on DELETE when built
        // with SQLITE_ENABLE_UPDATE_DELETE_LIMIT, which rusqlite's `bundled`
        // feature does not set — hence the rowid subquery form.
        // browse_cache_files has PK (snap, path), not INTEGER PRIMARY KEY, so it's
        // a rowid table and this works.
        //
        // Must run before the indexed_snapshots cleanup below in the same
        // transaction: if reversed, a marked snapshot's mapping row would be
        // dropped while its file rows still exist, making them unreachable by
        // every future query (they're looked up via `snap`, which comes from
        // this mapping) — an unrecoverable leak, not just a slower sweep.
        removed += tx
            .execute(
                "DELETE FROM browse_cache_files WHERE rowid IN (
                     SELECT rowid FROM browse_cache_files
                     WHERE snap IN (SELECT id FROM indexed_snapshots WHERE orphaned_at IS NOT NULL)
                     LIMIT ?1
                 )",
                params![max_rows as i64],
            )
            .map_err(|e| e.to_string())? as u64;

        // Retire marked snapshots that now have zero file rows left. A correlated
        // NOT EXISTS, not `id NOT IN (SELECT DISTINCT snap FROM browse_cache_files)` — the
        // IN form plans as a full scan of browse_cache_files (via idx_browse_files) on
        // every call, live rows included; on a large cache drained in 5,000-row batches
        // that's a full index scan per batch. NOT EXISTS plans as one indexed seek
        // (snap=?) per marked mapping instead — verified via EXPLAIN QUERY PLAN on the
        // real schema. Same result set either way: browse_cache_files.snap is
        // INTEGER NOT NULL, so there's no NULL-handling difference between the two forms.
        removed += tx
            .execute(
                "DELETE FROM indexed_snapshots
                 WHERE orphaned_at IS NOT NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM browse_cache_files f WHERE f.snap = indexed_snapshots.id
                   )",
                [],
            )
            .map_err(|e| e.to_string())? as u64;

        let more_remaining: bool = tx
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM browse_cache_files
                     WHERE snap IN (SELECT id FROM indexed_snapshots WHERE orphaned_at IS NOT NULL)
                 )",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(DrainBatch { rows_deleted: removed, more_remaining })
    }

    /// Remove only orphaned cache rows, leaving live caches intact, in a single
    /// synchronous call. Returns `(rows_deleted, db_size_bytes)`. Orphans are:
    ///   - `snapshots_cache` / `repo_stats_cache` rows whose `repo_id` no longer
    ///     exists in `repositories` (e.g. a deleted repo),
    ///   - `browse_cache_files` / `browse_cache_status` / `indexed_snapshots`
    ///     rows whose `snapshot_id` is not referenced by any remaining
    ///     `snapshots_cache` entry.
    ///
    /// Internally this is `mark_orphans` followed by `drain_orphans` looped to
    /// completion — the same primitives the `clean_cache` Tauri command uses, except
    /// this method runs the whole sweep in one call with no batching pause, so it's
    /// only appropriate for callers (this module's tests) that don't need to keep the
    /// `AppDb` connection mutex free while it runs. The async command wraps
    /// `mark_orphans`/`drain_orphans` itself instead, yielding between batches — see
    /// its doc comment.
    ///
    /// Only exercised directly by this module's tests today (`cargo clippy
    /// --all-targets` still flags it dead-code from the non-test lib target) — kept as
    /// a `pub fn` rather than `#[cfg(test)]`-gated since it's the natural non-batched
    /// building block any future non-UI caller (e.g. a CLI/debug command) would want.
    #[allow(dead_code)]
    pub fn clean_cache(&self) -> Result<(u64, u64), String> {
        let mut removed = self.mark_orphans()?;
        loop {
            let batch = self.drain_orphans(CLEAN_CACHE_BATCH_ROWS)?;
            removed += batch.rows_deleted;
            if !batch.more_remaining {
                break;
            }
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
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
                "INSERT INTO repositories
                 (id, name, path, password_nonce, password_ciphertext, read_only,
                  credentials_nonce, credentials_ciphertext)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    r.id,
                    r.name,
                    r.path,
                    r.password_nonce,
                    r.password_ciphertext,
                    r.read_only,
                    r.credentials_nonce,
                    r.credentials_ciphertext,
                ],
            )
            .map_err(|e| e.to_string())?;
        }

        for plan in plans {
            let paths_json = serde_json::to_string(&plan.paths).map_err(|e| e.to_string())?;
            let tags_json = serde_json::to_string(&plan.tags).map_err(|e| e.to_string())?;
            let excludes_json = serde_json::to_string(&plan.excludes).map_err(|e| e.to_string())?;
            let exclude_if_present_json =
                serde_json::to_string(&plan.exclude_if_present).map_err(|e| e.to_string())?;
            let webhooks_json =
                serde_json::to_string(&plan.webhooks).map_err(|e| e.to_string())?;
            let retention_json = plan
                .retention
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e: serde_json::Error| e.to_string())?;
            tx.execute(
                "INSERT INTO backup_plans
                 (id, name, repo_id, paths_json, tags_json, excludes_json, exclude_if_present_json, exclude_caches, retention_json, limit_upload, limit_download, webhooks_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    plan.id, plan.name, plan.repo_id, paths_json, tags_json, excludes_json,
                    exclude_if_present_json, plan.exclude_caches,
                    retention_json, plan.limit_upload, plan.limit_download, webhooks_json,
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
    pub read_only: bool,
    pub credentials_nonce: Option<Vec<u8>>,
    pub credentials_ciphertext: Option<Vec<u8>>,
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
    /// Logical size in bytes from restic's embedded backup summary
    /// (`summary.total_bytes_processed`); `None` when the snapshot carries no summary.
    size: Option<i64>,
}

fn parse_snapshot_rows(json: &str) -> Result<Vec<SnapshotRow>, String> {
    #[derive(Deserialize)]
    struct RawSummary {
        #[serde(default)]
        total_bytes_processed: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Raw {
        id: String,
        short_id: String,
        time: String,
        hostname: String,
        username: Option<String>,
        paths: Vec<String>,
        tags: Option<Vec<String>>,
        #[serde(default)]
        summary: Option<RawSummary>,
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
                size: r
                    .summary
                    .and_then(|s| s.total_bytes_processed)
                    .map(|v| v as i64),
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

/// Shared body behind both the "Clean Orphaned Data" button (`clean_cache`, `origin:
/// Manual`) and the automatic tick (`cache_warmer.rs`'s `maybe_run_cleanup`, `origin:
/// Background`): mark, then drain in `CLEAN_CACHE_BATCH_ROWS`-row batches, yielding the
/// `AppDb` connection mutex between batches so a large backlog (recorded in practice at
/// 100K+ rows) never freezes the app the way the old single-transaction sweep could. See
/// `AppDb::mark_orphans`/`drain_orphans` for the mechanism and `AppDb::clean_cache` for
/// the synchronous, non-batched equivalent used by tests. The `busy` guard on
/// `CleanupHandle` is what actually serializes a manual click against an automatic tick
/// (or either against itself) — whichever gets here first wins, the other bails out
/// immediately. Returns rows removed.
pub(crate) async fn run_cleanup(app: tauri::AppHandle, origin: TaskOrigin) -> Result<u64, String> {
    use std::sync::atomic::Ordering;

    let cleanup_handle = app.state::<CleanupHandle>();
    if cleanup_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A cleanup is already running".to_string());
    }
    struct BusyGuard<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&cleanup_handle.busy);
    cleanup_handle.cancelled.store(false, Ordering::SeqCst);

    let app_for_db = app.clone();
    let marked = tauri::async_runtime::spawn_blocking(move || {
        app_for_db.state::<AppDb>().mark_orphans()
    })
    .await
    .map_err(|e| e.to_string())??;

    let app_for_db = app.clone();
    let total = tauri::async_runtime::spawn_blocking(move || {
        app_for_db.state::<AppDb>().pending_orphan_row_count()
    })
    .await
    .map_err(|e| e.to_string())??;

    // repo_id is deliberately "" — cleanup is app-wide, not scoped to one repo. See
    // TaskKind::Cleanup's doc comment.
    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Cleanup,
        String::new(),
        None,
        origin,
        Some(cleanup_handle.current_task.clone()),
    );
    let progress = task_ctx.progress_emitter();
    // OperationCtx::new's Started event always carries progress: None (see
    // build_event) — items_total is already known at this point (from
    // pending_orphan_row_count above), so emit it immediately rather than
    // leaving the frontend at itemsTotal: 0 until the first batch round-trips.
    let mut removed = marked;
    progress.emit(TaskProgress {
        items_done: Some(removed),
        items_total: Some(total),
        ..Default::default()
    });

    let result: Result<(), String> = loop {
        if cleanup_handle.cancelled.load(Ordering::SeqCst) {
            break Ok(());
        }
        let app_for_db = app.clone();
        let batch = match tauri::async_runtime::spawn_blocking(move || {
            app_for_db.state::<AppDb>().drain_orphans(CLEAN_CACHE_BATCH_ROWS)
        })
        .await
        .map_err(|e| e.to_string())
        {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => break Err(e),
            Err(e) => break Err(e),
        };
        removed += batch.rows_deleted;
        progress.emit(TaskProgress {
            items_done: Some(removed),
            items_total: Some(total),
            ..Default::default()
        });
        if !batch.more_remaining {
            break Ok(());
        }
        // Release the blocking-pool thread so a command already queued on the
        // AppDb mutex gets scheduled before we come back around for the next
        // batch — the whole point of batching in the first place.
        tokio::task::yield_now().await;
    };

    let cancelled = cleanup_handle.cancelled.load(Ordering::SeqCst);
    match &result {
        Ok(_) if cancelled => task_ctx.cancelled(),
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result?;
    Ok(removed)
}

/// "Clean Orphaned Data" button — thin wrapper over `run_cleanup` with `origin: Manual`,
/// plus the DB-size readout the Settings UI shows afterward. Signature and return shape
/// (`rows_deleted`, `db_size_bytes`) are unchanged from before extraction, so the
/// `invoke.ts` wrapper and Settings UI need no changes.
#[tauri::command]
pub async fn clean_cache(app: tauri::AppHandle) -> Result<(u64, u64), String> {
    let removed = run_cleanup(app.clone(), TaskOrigin::Manual).await?;
    let size = tauri::async_runtime::spawn_blocking(move || app.state::<AppDb>().get_size())
        .await
        .map_err(|e| e.to_string())??;
    Ok((removed, size))
}

#[tauri::command]
pub async fn stop_cleanup(
    app: tauri::AppHandle,
    cleanup_handle: tauri::State<'_, CleanupHandle>,
) -> Result<(), String> {
    emit_cancelling(&app, &cleanup_handle.current_task);
    cleanup_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
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
    fn master_key_is_locked_reflects_set_and_clear() {
        let mk = MasterKey::new();
        assert!(mk.is_locked());
        mk.set([0u8; 32]).unwrap();
        assert!(!mk.is_locked());
        mk.clear().unwrap();
        assert!(mk.is_locked());
    }

    #[test]
    fn get_settings_with_empty_keys_returns_ok_empty_map() {
        // Would otherwise build `WHERE key IN ()` — a SQL syntax error.
        let db = test_db();
        let rows = db.get_settings(&[]).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn save_backup_plan_round_trips_exclude_if_present_and_exclude_caches() {
        let db = test_db();
        let plan = BackupPlan {
            id: "plan1".to_string(),
            name: "Daily".to_string(),
            repo_id: "repo1".to_string(),
            paths: vec!["/home".to_string()],
            tags: vec![],
            excludes: vec!["*.log".to_string()],
            exclude_if_present: vec![".nobackup".to_string(), "CACHEDIR.TAG".to_string()],
            exclude_caches: true,
            retention: None,
            limit_upload: None,
            limit_download: None,
            webhooks: vec![],
        };
        db.save_backup_plan(&plan).unwrap();

        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(
            plans[0].exclude_if_present,
            vec![".nobackup".to_string(), "CACHEDIR.TAG".to_string()]
        );
        assert!(plans[0].exclude_caches);

        // get_plans_for_ids shares the same read path — confirm it agrees.
        let by_id = db.get_plans_for_ids(&["plan1".to_string()]).unwrap();
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].exclude_if_present, plans[0].exclude_if_present);
        assert!(by_id[0].exclude_caches);
    }

    #[test]
    fn save_backup_plan_round_trips_webhooks() {
        let db = test_db();
        let plan = BackupPlan {
            id: "plan1".to_string(),
            name: "Daily".to_string(),
            repo_id: "repo1".to_string(),
            paths: vec!["/home".to_string()],
            tags: vec![],
            excludes: vec![],
            exclude_if_present: vec![],
            exclude_caches: false,
            retention: None,
            limit_upload: None,
            limit_download: None,
            webhooks: vec![
                PlanWebhook {
                    id: "w1".to_string(),
                    url: "https://discord.com/api/webhooks/x".to_string(),
                    provider: WebhookProvider::Discord,
                    stages: WebhookStages { started: true, completed: true, failed: false },
                    template: None,
                },
                PlanWebhook {
                    id: "w2".to_string(),
                    url: "https://hooks.example.com/x".to_string(),
                    provider: WebhookProvider::Generic,
                    stages: WebhookStages::default(),
                    template: None,
                },
                PlanWebhook {
                    id: "w3".to_string(),
                    url: "https://hooks.example.com/y".to_string(),
                    provider: WebhookProvider::Custom,
                    stages: WebhookStages::default(),
                    template: Some(r#"{"text": "{planName} {eventName}"}"#.to_string()),
                },
            ],
        };
        db.save_backup_plan(&plan).unwrap();

        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].webhooks, plan.webhooks);

        let by_id = db.get_plans_for_ids(&["plan1".to_string()]).unwrap();
        assert_eq!(by_id[0].webhooks, plan.webhooks);

        // The fire path's lookup agrees too.
        let (name, webhooks) = db.get_plan_webhooks("plan1").unwrap().unwrap();
        assert_eq!(name, "Daily");
        assert_eq!(webhooks, plan.webhooks);
    }

    #[test]
    fn old_schema_row_reads_back_empty_webhooks_and_unknown_plan_is_none() {
        // Simulates a plan row inserted before this migration (NULL webhooks_json) —
        // both the list read path and the fire-path lookup must tolerate it.
        let db = test_db();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO backup_plans (id, name, repo_id, paths_json, tags_json, excludes_json)
                 VALUES ('old-plan', 'Old', 'repo1', '[\"/home\"]', '[]', '[]')",
                [],
            )
            .unwrap();
        }
        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert!(plans[0].webhooks.is_empty());

        let (name, webhooks) = db.get_plan_webhooks("old-plan").unwrap().unwrap();
        assert_eq!(name, "Old");
        assert!(webhooks.is_empty());

        // A plan id that doesn't exist (e.g. deleted mid-backup) fires nothing.
        assert!(db.get_plan_webhooks("no-such-plan").unwrap().is_none());
    }

    #[test]
    fn old_schema_row_reads_back_empty_exclude_if_present_and_false_exclude_caches() {
        // Simulates a plan row inserted before this migration (NULL exclude_if_present_json,
        // default 0 exclude_caches) to confirm the read path tolerates it.
        let db = test_db();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO backup_plans (id, name, repo_id, paths_json, tags_json, excludes_json)
                 VALUES ('old-plan', 'Old', 'repo1', '[\"/home\"]', '[]', '[]')",
                [],
            )
            .unwrap();
        }
        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert!(plans[0].exclude_if_present.is_empty());
        assert!(!plans[0].exclude_caches);
    }

    #[test]
    fn import_bundle_writes_exclude_if_present_and_exclude_caches() {
        // Exercises the actual INSERT statement import_bundle runs (VALUES ?1..?11) —
        // a placeholder/column mismatch here would only ever surface on a real import.
        let db = test_db();
        let repo = ImportRepo {
            id: "repo1".to_string(),
            name: "Repo".to_string(),
            path: "/backups".to_string(),
            password_nonce: vec![],
            password_ciphertext: vec![],
            read_only: false,
            credentials_nonce: None,
            credentials_ciphertext: None,
        };
        let plan = BackupPlan {
            id: "plan1".to_string(),
            name: "Daily".to_string(),
            repo_id: "repo1".to_string(),
            paths: vec!["/home".to_string()],
            tags: vec![],
            excludes: vec!["*.log".to_string()],
            exclude_if_present: vec![".nobackup".to_string()],
            exclude_caches: true,
            retention: None,
            limit_upload: None,
            limit_download: None,
            webhooks: vec![],
        };
        db.import_bundle(&[repo], &[plan], &[]).unwrap();

        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].exclude_if_present, vec![".nobackup".to_string()]);
        assert!(plans[0].exclude_caches);
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

    fn test_file_entry(path: &str) -> FileEntry {
        FileEntry {
            name: name_of(path),
            path: path.to_string(),
            entry_type: "file".to_string(),
            size: Some(1),
            mtime: None,
            mode: None,
        }
    }

    // AppDb::get's three-way return semantics — Some(non-empty), Some(empty), None — are
    // load-bearing: `Some(vec![])` means "a real, empty directory," while `None` means "not
    // cached, go fetch." Conflating the two would make an empty directory look like a cache
    // miss (or vice versa). See the browse_cache_files cross-repo aliasing note this test sits
    // near in spirit — get()'s own semantics here are a separate, narrower concern.
    #[test]
    fn get_returns_some_empty_vec_for_a_fully_indexed_empty_directory() {
        let db = test_db();
        db.set_browse_status("repoA", "snap1", "complete").unwrap();
        // No browse_cache_files rows written at all for this snapshot/path — a genuinely
        // empty directory, not an unindexed one.
        let result = db.get("repoA", "snap1", None).unwrap();
        assert_eq!(result.as_ref().map(|v| v.len()), Some(0));
    }

    #[test]
    fn get_returns_some_for_a_partially_indexed_directory_with_rows() {
        let db = test_db();
        // Deliberately no "complete" status row — partial indexing, but this directory does
        // have cached rows (e.g. a manual index that was cancelled partway through).
        db.set("snap1", None, &[test_file_entry("a.txt")]).unwrap();
        let result = db.get("repoA", "snap1", None).unwrap();
        assert_eq!(result.as_ref().map(|v| v.len()), Some(1));
    }

    #[test]
    fn get_returns_none_for_a_partially_indexed_directory_with_no_rows() {
        let db = test_db();
        // Neither a "complete" status nor any cached rows for this directory — a genuine
        // cache miss, distinct from the fully-indexed-empty case above.
        let result = db.get("repoA", "snap1", None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn set_stats_returns_and_persists_cached_at() {
        let db = test_db();

        let stats1 = ResticStats {
            total_size: 100,
            total_file_count: 5,
            snapshots_count: 2,
            raw_size: Some(40),
            cached_at: None,
        };
        let ts1 = db.set_stats("repoA", &stats1).unwrap();
        let got = db.get_stats("repoA").unwrap().unwrap();
        assert_eq!((got.total_size, got.total_file_count, got.snapshots_count), (100, 5, 2));
        assert_eq!(got.raw_size, Some(40));
        assert_eq!(got.cached_at, Some(ts1));

        // A later set_stats overwrites the value and advances cached_at (or at least
        // never goes backwards — timestamp() is second-resolution, so two calls in the
        // same test can legitimately land on the same second).
        let stats2 = ResticStats {
            total_size: 200,
            total_file_count: 8,
            snapshots_count: 3,
            raw_size: None,
            cached_at: None,
        };
        let ts2 = db.set_stats("repoA", &stats2).unwrap();
        assert!(ts2 >= ts1);
        let got = db.get_stats("repoA").unwrap().unwrap();
        assert_eq!((got.total_size, got.total_file_count, got.snapshots_count), (200, 8, 3));
        // A refresh whose raw-data call failed overwrites the previous raw_size with
        // None — matches fetch_and_cache_stats always writing whatever it just fetched
        // (or didn't) rather than preserving a stale raw_size from an earlier cycle.
        assert_eq!(got.raw_size, None);
        assert_eq!(got.cached_at, Some(ts2));
    }

    #[test]
    fn get_stats_legacy_row_without_raw_size_reads_back_as_none() {
        // Simulates a pre-migration row: INSERT directly, bypassing set_stats, into only
        // the columns that existed before raw_size was added.
        let db = test_db();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO repo_stats_cache
                 (repo_id, total_size, total_file_count, snapshots_count, cached_at)
                 VALUES ('repoA', 100, 5, 2, 12345)",
                [],
            )
            .unwrap();
        }
        let got = db.get_stats("repoA").unwrap().unwrap();
        assert_eq!(got.raw_size, None);
        assert_eq!(got.cached_at, Some(12345));
    }

    #[test]
    fn init_schema_adds_raw_size_column_to_existing_repo_stats_cache() {
        // Simulate an install that already has `repo_stats_cache` from before `raw_size`
        // existed — running init_schema (as every app startup does) must add the column
        // via the ALTER TABLE migration rather than erroring on the pre-existing table.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE repo_stats_cache (
                 repo_id          TEXT PRIMARY KEY,
                 total_size       INTEGER NOT NULL,
                 total_file_count INTEGER NOT NULL,
                 snapshots_count  INTEGER NOT NULL,
                 cached_at        INTEGER NOT NULL
             );
             INSERT INTO repo_stats_cache VALUES ('repoA', 100, 5, 2, 12345);",
        )
        .unwrap();

        AppDb::init_schema(&conn).expect("init_schema should migrate the existing table");

        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(repo_stats_cache)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(cols.contains(&"raw_size".to_string()));

        // Pre-existing row survives the migration with raw_size defaulting to NULL.
        let raw_size: Option<i64> = conn
            .query_row("SELECT raw_size FROM repo_stats_cache WHERE repo_id = 'repoA'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw_size, None);
    }

    #[test]
    fn init_schema_adds_orphaned_at_column_to_existing_indexed_snapshots() {
        // Simulate an install that already has `indexed_snapshots` from before
        // `orphaned_at` existed (the pre-clean_cache-batching shape).
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE indexed_snapshots (
                 id           INTEGER PRIMARY KEY,
                 snapshot_id  TEXT NOT NULL UNIQUE
             );
             INSERT INTO indexed_snapshots (snapshot_id) VALUES ('aaaa111100000000');",
        )
        .unwrap();

        AppDb::init_schema(&conn).expect("init_schema should migrate the existing table");
        // Idempotent: a second call (every subsequent app startup) must not error.
        AppDb::init_schema(&conn).expect("init_schema must be idempotent");

        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(indexed_snapshots)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(cols.contains(&"orphaned_at".to_string()));

        let orphaned_at: Option<i64> = conn
            .query_row(
                "SELECT orphaned_at FROM indexed_snapshots WHERE snapshot_id = 'aaaa111100000000'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphaned_at, None, "pre-existing row must default to not-orphaned");
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

    fn snapshot_json(id: &str, tags: Option<&[&str]>) -> String {
        let tags_json = match tags {
            Some(t) => serde_json::to_string(t).unwrap(),
            None => "null".to_string(),
        };
        format!(
            r#"{{"id":"{id}","short_id":"{id}","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"],"tags":{tags_json}}}"#
        )
    }

    #[test]
    fn remove_snapshot_from_cache_deletes_only_the_targeted_row() {
        let db = test_db();
        let repo1 = "repo1";
        let repo2 = "repo2";
        seed_repo(&db, repo1);
        seed_repo(&db, repo2);
        db.set_snapshots(
            repo1,
            &format!("[{},{}]", snapshot_json("aaaa111100000000", None), snapshot_json("bbbb222200000000", None)),
        )
        .unwrap();
        seed_snapshot(&db, repo2, "cccc333300000000");

        db.remove_snapshot_from_cache(repo1, "aaaa111100000000").unwrap();

        let remaining: Vec<String> = db.get_snapshots_vec(repo1).unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(remaining, vec!["bbbb222200000000".to_string()]);
        // Untouched: another repo's row, and this same call must not touch
        // indexed_snapshots/browse_cache_files/browse_cache_status at all.
        assert_eq!(db.get_snapshots_vec(repo2).unwrap().len(), 1);
    }

    #[test]
    fn set_snapshots_diff_deletes_only_dropped_ids() {
        let db = test_db();
        seed_repo(&db, "repo1");
        db.set_snapshots(
            "repo1",
            &format!(
                "[{},{}]",
                snapshot_json("aaaa111100000000", None),
                snapshot_json("bbbb222200000000", None)
            ),
        )
        .unwrap();

        // Second call drops bbbb, keeps aaaa.
        db.set_snapshots("repo1", &format!("[{}]", snapshot_json("aaaa111100000000", None))).unwrap();

        let remaining: Vec<String> = db.get_snapshots_vec("repo1").unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(remaining, vec!["aaaa111100000000".to_string()]);
    }

    /// The single-drop case above can't catch an off-by-one in the dynamic placeholder
    /// list (the `repo_id` param bound first, then N dropped-id params) — this drops two
    /// of three in one call to exercise that binding with more than one item.
    #[test]
    fn set_snapshots_diff_deletes_multiple_dropped_ids_in_one_call() {
        let db = test_db();
        seed_repo(&db, "repo1");
        db.set_snapshots(
            "repo1",
            &format!(
                "[{},{},{}]",
                snapshot_json("aaaa111100000000", None),
                snapshot_json("bbbb222200000000", None),
                snapshot_json("cccc333300000000", None)
            ),
        )
        .unwrap();

        // Drops aaaa and cccc, keeps bbbb.
        db.set_snapshots("repo1", &format!("[{}]", snapshot_json("bbbb222200000000", None))).unwrap();

        let remaining: Vec<String> = db.get_snapshots_vec("repo1").unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(remaining, vec!["bbbb222200000000".to_string()]);
    }

    #[test]
    fn set_snapshots_diff_adds_new_ids_without_touching_others() {
        let db = test_db();
        seed_repo(&db, "repo1");
        db.set_snapshots("repo1", &format!("[{}]", snapshot_json("aaaa111100000000", None))).unwrap();

        db.set_snapshots(
            "repo1",
            &format!(
                "[{},{}]",
                snapshot_json("aaaa111100000000", None),
                snapshot_json("bbbb222200000000", None)
            ),
        )
        .unwrap();

        let mut ids: Vec<String> = db.get_snapshots_vec("repo1").unwrap().into_iter().map(|s| s.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["aaaa111100000000".to_string(), "bbbb222200000000".to_string()]);
    }

    /// Exercises the `ON CONFLICT` upsert path specifically: calling `set_snapshots` again
    /// with the exact same listing must not error (no duplicate-key failure) and must leave
    /// exactly the same rows in place.
    #[test]
    fn set_snapshots_is_idempotent_on_an_unchanged_listing() {
        let db = test_db();
        seed_repo(&db, "repo1");
        let json = format!(
            "[{},{}]",
            snapshot_json("aaaa111100000000", None),
            snapshot_json("bbbb222200000000", None)
        );
        db.set_snapshots("repo1", &json).unwrap();
        db.set_snapshots("repo1", &json).unwrap();

        let mut ids: Vec<String> = db.get_snapshots_vec("repo1").unwrap().into_iter().map(|s| s.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["aaaa111100000000".to_string(), "bbbb222200000000".to_string()]);
    }

    /// Pins the reason every fetched row is always upserted rather than skipped when its id
    /// already exists: a tag added out-of-band (e.g. `restic tag` run outside the app) on an
    /// id that neither added nor dropped must still be picked up on the next refresh.
    #[test]
    fn set_snapshots_picks_up_a_tag_change_on_an_existing_id() {
        let db = test_db();
        seed_repo(&db, "repo1");
        db.set_snapshots("repo1", &format!("[{}]", snapshot_json("aaaa111100000000", None))).unwrap();
        assert_eq!(db.get_snapshots_vec("repo1").unwrap()[0].tags, None);

        db.set_snapshots(
            "repo1",
            &format!("[{}]", snapshot_json("aaaa111100000000", Some(&["daily"]))),
        )
        .unwrap();

        let snapshots = db.get_snapshots_vec("repo1").unwrap();
        assert_eq!(snapshots.len(), 1, "must not duplicate the row");
        assert_eq!(snapshots[0].tags, Some(vec!["daily".to_string()]));
    }

    /// `Snapshot.size` is read from restic's nested `summary.total_bytes_processed`, is
    /// `None` when the snapshot carries no summary, and survives the diff-based upsert path
    /// (a later refresh that adds a summary must fill it in).
    #[test]
    fn set_snapshots_stores_summary_size_and_leaves_it_none_when_absent() {
        let db = test_db();
        seed_repo(&db, "repo1");
        let with_summary = r#"{"id":"aaaa111100000000","short_id":"aaaa1111","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"],"tags":null,"summary":{"total_bytes_processed":4096}}"#;
        let without_summary = snapshot_json("bbbb222200000000", None);
        db.set_snapshots("repo1", &format!("[{with_summary},{without_summary}]")).unwrap();

        let by_id = |id: &str| {
            db.get_snapshots_vec("repo1")
                .unwrap()
                .into_iter()
                .find(|s| s.id == id)
                .unwrap()
        };
        assert_eq!(by_id("aaaa111100000000").size, Some(4096));
        assert_eq!(by_id("bbbb222200000000").size, None);

        // A later listing that gains a summary for bbbb backfills the size via the upsert.
        let bbbb_with_summary = r#"{"id":"bbbb222200000000","short_id":"bbbb2222","time":"2024-01-01T00:00:00Z","hostname":"host","paths":["/home"],"tags":null,"summary":{"total_bytes_processed":8192}}"#;
        db.set_snapshots("repo1", &format!("[{with_summary},{bbbb_with_summary}]")).unwrap();
        assert_eq!(by_id("bbbb222200000000").size, Some(8192));
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

    // ── backend credentials ─────────────────────────────────────────────────

    #[test]
    fn credentials_round_trip_through_add_repo_and_get_full_repo() {
        let db = test_db();
        let key = [7u8; 32];
        let (pw_nonce, pw_ct) = super::crypto::encrypt(&key, b"pw").unwrap();
        let creds = vec![
            Credential { key: "B2_ACCOUNT_ID".to_string(), value: "id".to_string() },
            Credential { key: "B2_ACCOUNT_KEY".to_string(), value: "secret".to_string() },
        ];
        let (cred_nonce, cred_ct) = encode_credentials(&key, &creds).unwrap();
        db.add_repo(
            "r1",
            "Repo",
            "b2:bucket:path",
            &pw_nonce,
            &pw_ct,
            false,
            cred_nonce.as_deref(),
            cred_ct.as_deref(),
        )
        .unwrap();

        let full = db.get_full_repo("r1", &key).unwrap();
        assert_eq!(full.password, "pw");
        let mut got: Vec<(String, String)> =
            full.credentials.iter().map(|c| (c.key.clone(), c.value.clone())).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("B2_ACCOUNT_ID".to_string(), "id".to_string()),
                ("B2_ACCOUNT_KEY".to_string(), "secret".to_string()),
            ]
        );
    }

    #[test]
    fn null_credential_columns_read_back_as_empty_vec() {
        // The ambient-mode invariant: a row with NULL credential columns — every
        // pre-existing repo, and any new repo created without stored credentials —
        // must decode to an empty credentials list, never an error or a default value.
        let db = test_db();
        let key = [7u8; 32];
        add_repo_encrypted(&db, "r1", "S3 Repo", "s3:s3.amazonaws.com/bucket", "pw", &key);

        let full = db.get_full_repo("r1", &key).unwrap();
        assert_eq!(full.password, "pw");
        assert!(full.credentials.is_empty());
    }

    #[test]
    fn update_repo_secrets_changing_password_preserves_credentials() {
        let db = test_db();
        let key = [7u8; 32];
        add_repo_with_credentials(
            &db,
            "r1",
            "b2:bucket:path",
            "old-pw",
            &[("B2_ACCOUNT_ID", "id"), ("B2_ACCOUNT_KEY", "key")],
            &key,
        );

        db.update_repo_secrets("r1", &key, Some("new-pw".to_string()), None).unwrap();

        let full = db.get_full_repo("r1", &key).unwrap();
        assert_eq!(full.password, "new-pw");
        assert_eq!(full.credentials.len(), 2);
    }

    #[test]
    fn update_repo_secrets_changing_credentials_preserves_password() {
        let db = test_db();
        let key = [7u8; 32];
        add_repo_encrypted(&db, "r1", "Repo", "b2:bucket:path", "pw", &key);

        let new_creds = vec![Credential { key: "B2_ACCOUNT_ID".to_string(), value: "id".to_string() }];
        db.update_repo_secrets("r1", &key, None, Some(new_creds)).unwrap();

        let full = db.get_full_repo("r1", &key).unwrap();
        assert_eq!(full.password, "pw");
        assert_eq!(full.credentials.len(), 1);
        assert_eq!(full.credentials[0].key, "B2_ACCOUNT_ID");
    }

    #[test]
    fn update_repo_secrets_with_empty_vec_clears_credentials_to_ambient_mode() {
        let db = test_db();
        let key = [7u8; 32];
        add_repo_with_credentials(
            &db,
            "r1",
            "b2:bucket:path",
            "pw",
            &[("B2_ACCOUNT_ID", "id"), ("B2_ACCOUNT_KEY", "key")],
            &key,
        );

        db.update_repo_secrets("r1", &key, None, Some(vec![])).unwrap();

        let full = db.get_full_repo("r1", &key).unwrap();
        assert!(full.credentials.is_empty());
    }

    #[test]
    fn get_repo_credentials_returns_values() {
        let db = test_db();
        let key = [7u8; 32];
        add_repo_with_credentials(
            &db,
            "r1",
            "b2:bucket:path",
            "pw",
            &[("B2_ACCOUNT_ID", "id"), ("B2_ACCOUNT_KEY", "key")],
            &key,
        );

        let mut creds = db.get_repo_credentials("r1", &key).unwrap();
        creds.sort();
        assert_eq!(
            creds,
            vec![
                ("B2_ACCOUNT_ID".to_string(), "id".to_string()),
                ("B2_ACCOUNT_KEY".to_string(), "key".to_string()),
            ]
        );
    }

    #[test]
    fn get_repo_credentials_returns_empty_vec_for_ambient_repo() {
        // Same invariant as the row-read path: a repo with NULL credential columns
        // (every pre-existing repo, and any new repo saved without credentials) reads
        // back as an empty list — never an error.
        let db = test_db();
        let key = [7u8; 32];
        add_repo_encrypted(&db, "r1", "S3 Repo", "s3:s3.amazonaws.com/bucket", "pw", &key);
        assert!(db.get_repo_credentials("r1", &key).unwrap().is_empty());
    }

    // ── rotate_master_key ───────────────────────────────────────────────────

    fn make_key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn add_repo_encrypted(db: &AppDb, id: &str, name: &str, path: &str, password: &str, key: &[u8; 32]) {
        let (nonce, ct) = super::crypto::encrypt(key, password.as_bytes()).unwrap();
        db.add_repo(id, name, path, &nonce, &ct, false, None, None).unwrap();
    }

    fn add_repo_with_credentials(
        db: &AppDb,
        id: &str,
        path: &str,
        password: &str,
        credentials: &[(&str, &str)],
        key: &[u8; 32],
    ) {
        let (nonce, ct) = super::crypto::encrypt(key, password.as_bytes()).unwrap();
        let creds: Vec<Credential> = credentials
            .iter()
            .map(|(k, v)| Credential { key: k.to_string(), value: v.to_string() })
            .collect();
        let (cred_nonce, cred_ct) = encode_credentials(key, &creds).unwrap();
        db.add_repo(
            id,
            "Repo",
            path,
            &nonce,
            &ct,
            false,
            cred_nonce.as_deref(),
            cred_ct.as_deref(),
        )
        .unwrap();
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
    fn rotate_master_key_preserves_credentials() {
        let db = test_db();
        let old_key = make_key(1);
        let new_key = make_key(2);

        add_repo_with_credentials(
            &db,
            "r1",
            "b2:my-bucket:restic",
            "pw",
            &[("B2_ACCOUNT_ID", "id123"), ("B2_ACCOUNT_KEY", "key456")],
            &old_key,
        );

        let salt = [0u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        db.rotate_master_key(&old_key, &new_key, &salt, &vn, &vct).unwrap();

        let r1 = db.get_full_repo("r1", &new_key).unwrap();
        assert_eq!(r1.password, "pw");
        let mut creds: Vec<(String, String)> =
            r1.credentials.iter().map(|c| (c.key.clone(), c.value.clone())).collect();
        creds.sort();
        assert_eq!(
            creds,
            vec![
                ("B2_ACCOUNT_ID".to_string(), "id123".to_string()),
                ("B2_ACCOUNT_KEY".to_string(), "key456".to_string()),
            ]
        );
    }

    #[test]
    fn rotate_master_key_rolls_back_atomically_on_undecryptable_credentials() {
        // A corrupted/undecryptable credentials blob must fail the whole rotation
        // rather than commit a partial result (e.g. password re-encrypted, credentials
        // left broken). Proven by checking the password_nonce/password_ciphertext
        // columns are byte-for-byte unchanged afterward — nothing committed. This is
        // the same all-or-nothing property the post-rotation verification pass exists
        // to guarantee more generally, for any secret field `decode_secrets` covers,
        // not just this case.
        //
        // Reads the raw columns directly rather than through `get_full_repo`: the
        // corruption below is applied outside any transaction (a real
        // UPDATE, committed immediately), so it permanently breaks the credentials
        // blob regardless of what rotate_master_key does — decoding via
        // `decode_secrets` would fail on the (deliberately, permanently) corrupted
        // credentials even when the password itself was never touched.
        let db = test_db();
        let old_key = make_key(1);
        let new_key = make_key(2);

        add_repo_with_credentials(
            &db,
            "r1",
            "b2:my-bucket:restic",
            "pw",
            &[("B2_ACCOUNT_ID", "id123"), ("B2_ACCOUNT_KEY", "key456")],
            &old_key,
        );

        let before: (Vec<u8>, Vec<u8>) = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT password_nonce, password_ciphertext FROM repositories WHERE id = 'r1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };

        // Directly corrupt the credentials blob to simulate it having been left under
        // a key that is neither old_key nor new_key (standing in for "never
        // re-encrypted").
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE repositories SET credentials_ciphertext = X'DEADBEEF' WHERE id = 'r1'",
                [],
            )
            .unwrap();
        }

        let salt = [0u8; 16];
        let (vn, vct) = super::crypto::encrypt(&new_key, b"verified").unwrap();
        assert!(db.rotate_master_key(&old_key, &new_key, &salt, &vn, &vct).is_err());

        // Nothing committed — the password columns are exactly what they were before
        // the rotation attempt (still under old_key, never touched).
        let after: (Vec<u8>, Vec<u8>) = {
            let conn = db.conn.lock().unwrap();
            conn.query_row(
                "SELECT password_nonce, password_ciphertext FROM repositories WHERE id = 'r1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(before, after);
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

    fn sample_file_entry() -> FileEntry {
        FileEntry {
            name: "secret.txt".to_string(),
            path: "/home/secret.txt".to_string(),
            entry_type: "file".to_string(),
            size: Some(123),
            mtime: None,
            mode: None,
        }
    }

    /// Pins the contract `remove_repo`'s doc comment relies on: it no longer cascades
    /// into `browse_cache_files`/`indexed_snapshots` directly, but the rows it leaves
    /// behind must be exactly what `clean_cache`'s orphan sweep picks up next.
    #[test]
    fn remove_repo_leaves_file_rows_for_clean_cache() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();

        assert_eq!(count_rows(&db, "browse_cache_files"), 1);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);

        db.remove_repo("repo1").unwrap();

        // Directly repo_id-keyed rows are gone immediately.
        assert_eq!(count_rows(&db, "repositories"), 0);
        assert_eq!(count_rows(&db, "snapshots_cache"), 0);
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);

        // File rows are left behind, now orphaned, for clean_cache to sweep.
        assert_eq!(count_rows(&db, "browse_cache_files"), 1);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);

        let (removed, _size) = db.clean_cache().unwrap();
        assert!(removed >= 2, "expected clean_cache to remove the orphaned file rows, got {removed}");
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
    }

    /// `browse_cache_files`/`indexed_snapshots` are keyed by `snapshot_id` alone, with
    /// no `repo_id` column — two repos that happen to share a snapshot id (e.g. the
    /// same underlying repo added twice) share those index rows too. Removing one repo
    /// must not delete indexing the other repo still relies on; only clean_cache's
    /// "nothing left references this snapshot_id" sweep may do that.
    #[test]
    fn remove_repo_keeps_file_rows_shared_with_another_repo() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "shared00000000000");
        seed_snapshot(&db, "repo2", "shared00000000000");
        db.set_browse_status("repo1", "shared00000000000", "complete").unwrap();
        db.set_browse_status("repo2", "shared00000000000", "complete").unwrap();
        db.insert_browse_files("shared00000000000", &[sample_file_entry()]).unwrap();

        db.remove_repo("repo1").unwrap();

        // repo2's snapshots_cache row still references the shared snapshot_id, so
        // clean_cache must leave the file rows alone.
        let (removed, _size) = db.clean_cache().unwrap();
        assert_eq!(removed, 0, "file rows still referenced by repo2 must survive");
        assert_eq!(count_rows(&db, "browse_cache_files"), 1);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
    }

    // ── mark_orphans / drain_orphans (batched clean_cache primitives) ─────────

    fn file_entries(n: usize) -> Vec<FileEntry> {
        (0..n)
            .map(|i| FileEntry {
                name: format!("f{i}.txt"),
                path: format!("/home/f{i}.txt"),
                entry_type: "file".to_string(),
                size: Some(1),
                mtime: None,
                mode: None,
            })
            .collect()
    }

    fn orphaned_at_of(db: &AppDb, snapshot_id: &str) -> Option<i64> {
        db.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT orphaned_at FROM indexed_snapshots WHERE snapshot_id = ?1",
                rusqlite::params![snapshot_id],
                |r| r.get(0),
            )
            .unwrap()
    }

    #[test]
    fn mark_orphans_stamps_and_drops_status_without_touching_file_rows() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();

        assert!(orphaned_at_of(&db, "aaaa111100000000").is_none());
        db.mark_orphans().unwrap();
        assert!(orphaned_at_of(&db, "aaaa111100000000").is_some());
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
        // File rows untouched by mark_orphans — that's drain_orphans's job.
        assert_eq!(count_rows(&db, "browse_cache_files"), 1);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
    }

    #[test]
    fn mark_orphans_is_idempotent() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();

        db.mark_orphans().unwrap();
        let first_stamp = orphaned_at_of(&db, "aaaa111100000000");
        assert_eq!(db.mark_orphans().unwrap(), 0, "second call should mark 0 more");
        assert_eq!(
            orphaned_at_of(&db, "aaaa111100000000"),
            first_stamp,
            "an existing mark must not be restamped"
        );
    }

    #[test]
    fn mark_orphans_never_marks_a_snapshot_shared_with_another_repo() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "shared00000000000");
        seed_snapshot(&db, "repo2", "shared00000000000");
        db.insert_browse_files("shared00000000000", &[sample_file_entry()]).unwrap();

        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();

        assert!(
            orphaned_at_of(&db, "shared00000000000").is_none(),
            "still referenced by repo2's snapshots_cache — must not be marked"
        );
    }

    #[test]
    fn mark_orphans_unmarks_a_resurrected_snapshot() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();
        assert!(orphaned_at_of(&db, "aaaa111100000000").is_some());

        // Repo re-added, snapshot re-appears in snapshots_cache.
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.mark_orphans().unwrap();

        assert!(orphaned_at_of(&db, "aaaa111100000000").is_none());
        // Its status row was dropped when it was marked and isn't restored —
        // resurrected snapshots re-index rather than being trusted as still cached.
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
    }

    #[test]
    fn intern_snapshot_via_insert_browse_files_clears_an_orphan_mark() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();
        assert!(orphaned_at_of(&db, "aaaa111100000000").is_some());

        // Simulate resurrection + re-indexing while a drain would still be pending:
        // repo re-added, snapshot back in snapshots_cache, then indexed again.
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &file_entries(2)).unwrap();

        assert!(
            orphaned_at_of(&db, "aaaa111100000000").is_none(),
            "re-indexing a resurrected snapshot must clear its orphan mark immediately, \
             not wait for the next mark_orphans pass"
        );
        // A drain running right now must not delete the file rows indexing just wrote
        // (the original sample_file_entry() row plus the 2 from file_entries(2)).
        let batch = db.drain_orphans(100).unwrap();
        assert_eq!(batch.rows_deleted, 0);
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);
    }

    #[test]
    fn intern_snapshot_via_insert_browse_files_is_unaffected_for_an_unmarked_snapshot() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        let id_before = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1",
                rusqlite::params!["aaaa111100000000"],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();

        // Indexing the same, still-live snapshot again (e.g. a re-index) must not
        // duplicate the row or disturb an already-NULL mark.
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();

        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
        let id_after = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1",
                rusqlite::params!["aaaa111100000000"],
                |r| r.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(id_before, id_after);
        assert!(orphaned_at_of(&db, "aaaa111100000000").is_none());
    }

    /// Pins the retire-mid-run guard in `insert_browse_files_chunk` — including its
    /// (id, snapshot_id) keying: once a drain has retired the mapping an in-flight index run
    /// resolved `snap` to, a later chunk from that same run must abort rather than write rows
    /// no query and no sweep can ever see again, **even when a different snapshot has since
    /// been interned onto the recycled rowid**. Reachable only at chunk granularity — a fresh
    /// `insert_browse_files` call would re-intern (the upsert above) and legitimately succeed —
    /// which is why the test drives the chunk helper directly instead.
    #[test]
    fn insert_browse_files_chunk_aborts_when_the_mapping_was_retired_mid_run() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();
        let snap = {
            let conn = db.conn.lock().unwrap();
            AppDb::snap_id_of(&conn, "aaaa111100000000")
                .unwrap()
                .expect("mapping interned above")
        };

        // Orphan the snapshot, then sweep to completion — the drain retires the mapping,
        // exactly as it would mid-run if the snapshot left snapshots_cache while this index
        // was still in flight (external forget + refresh, or repo removal).
        db.remove_repo("repo1").unwrap();
        db.clean_cache().unwrap();
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);

        // Intern a different snapshot afterwards. The retired mapping held rowid 1 on this
        // fresh test DB, the table is now empty, and INTEGER PRIMARY KEY without
        // AUTOINCREMENT recycles the freed max rowid — so "bbbb…" lands on the *same* id the
        // still-"in-flight" run cached. A guard keyed on id alone would pass here.
        db.insert_browse_files("bbbb222200000000", &file_entries(1)).unwrap();
        let recycled = {
            let conn = db.conn.lock().unwrap();
            AppDb::snap_id_of(&conn, "bbbb222200000000").unwrap()
        };
        assert_eq!(
            recycled,
            Some(snap),
            "the fresh mapping should recycle the retired rowid"
        );
        assert_eq!(count_rows(&db, "browse_cache_files"), 1);

        // A late chunk from the still-"in-flight" run: must error, must write nothing —
        // in particular nothing under the recycled id's new owner.
        let err = db
            .insert_browse_files_chunk(snap, "aaaa111100000000", &file_entries(2))
            .unwrap_err();
        assert!(err.contains("index aborted"), "unexpected error: {err}");
        assert_eq!(
            count_rows(&db, "browse_cache_files"),
            1,
            "no rows may land against another snapshot's recycled mapping id"
        );
    }

    /// Pins the single-repo shape: one evict call leaves none of the three row kinds behind,
    /// crash-atomically (one transaction — see evict's doc for why the transaction is about
    /// crash/error atomicity, not interleaving). The cross-repo sharing branch is pinned by
    /// `evict_keeps_shared_rows_while_another_repo_still_references_the_snapshot` below.
    #[test]
    fn evict_removes_files_mapping_and_status_in_one_call() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
        assert_eq!(count_rows(&db, "browse_cache_status"), 1);

        db.evict("repo1", "aaaa111100000000").unwrap();

        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
        // And nothing dangles for a later sweep to disagree about.
        assert!(!db.has_cleanup_work().unwrap());
    }

    /// Pins evict's cross-repo sharing guard: after restic copy/mirror the same snapshot id
    /// can live in two repos, and `browse_cache_files`/`indexed_snapshots` are shared (no
    /// repo_id column). The guard keys on the *other repo's status rows* — the consumers of
    /// the shared rows — so repo2's 'complete' index keeps them alive; repo1's clear removes
    /// only its own status. Once repo2 clears too (no status rows remain), the shared rows
    /// are reclaimed. See `evict_keys_the_sharing_guard_on_status_rows_not_listings` for why
    /// the key is status rows and not `snapshots_cache` listings.
    #[test]
    fn evict_keeps_shared_rows_while_another_repo_still_references_the_snapshot() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        seed_snapshot(&db, "repo2", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.set_browse_status("repo2", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);

        // Clear repo1's index: only repo1's status row may go; repo2 is untouched.
        db.evict("repo1", "aaaa111100000000").unwrap();
        assert!(!db
            .get_browse_status("repo1")
            .unwrap()
            .contains_key("aaaa111100000000"));
        assert_eq!(
            db.get_browse_status("repo2").unwrap().get("aaaa111100000000"),
            Some(&"complete".to_string())
        );
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
        assert!(!db.has_cleanup_work().unwrap());

        // repo2's clear is the last reference: everything goes, listing or not.
        db.evict("repo2", "aaaa111100000000").unwrap();
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
        assert!(!db.has_cleanup_work().unwrap());
    }

    /// Pins *which* rows evict's sharing guard keys on — the two cases a listings-keyed
    /// guard got exactly backwards, plus the non-complete statuses that must not count:
    /// - A repo whose 'complete' status survives its own snapshot forget
    ///   (`remove_snapshot_from_cache` deletes only the listing) still depends on the
    ///   shared rows — a clear in the other repo must keep them, or the forget-repo
    ///   browses a permanently empty tree nothing retries or sweeps.
    /// - A repo that merely *lists* the snapshot (never indexed it) depends on nothing —
    ///   its listing must not keep the rows alive forever for no reader.
    #[test]
    fn evict_keys_the_sharing_guard_on_status_rows_not_listings() {
        // Status without a listing: repo2 forgot its copy, but its 'complete' index remains.
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        seed_snapshot(&db, "repo2", "aaaa111100000000");
        db.remove_snapshot_from_cache("repo2", "aaaa111100000000")
            .unwrap();
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.set_browse_status("repo2", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(2)).unwrap();

        db.evict("repo1", "aaaa111100000000").unwrap();

        assert_eq!(
            db.get_browse_status("repo2").unwrap().get("aaaa111100000000"),
            Some(&"complete".to_string()),
            "repo2's surviving index must keep the shared rows it reads"
        );
        assert_eq!(count_rows(&db, "browse_cache_files"), 2);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);

        // Listing without a status: repo2 lists the snapshot but never indexed it —
        // repo1's clear reclaims the rows outright.
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        seed_snapshot(&db, "repo2", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(2)).unwrap();

        db.evict("repo1", "aaaa111100000000").unwrap();

        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);
        assert!(!db.has_cleanup_work().unwrap());

        // A non-complete status in the other repo pins nothing: 'pending' has no
        // readable index (its run failed or never ran), so the shared rows are
        // reclaimed rather than pinned forever — repo2's retryable status survives,
        // and its next run simply re-indexes from scratch.
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_repo(&db, "repo2");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        seed_snapshot(&db, "repo2", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.set_browse_status("repo2", "aaaa111100000000", "pending").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(2)).unwrap();

        db.evict("repo1", "aaaa111100000000").unwrap();

        assert_eq!(
            count_rows(&db, "browse_cache_files"),
            0,
            "a 'pending' other repo is not a reader"
        );
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
        assert_eq!(
            db.get_browse_status("repo2").unwrap().get("aaaa111100000000"),
            Some(&"pending".to_string())
        );
    }

    /// Pins `set_browse_status_if_present`'s gate: an index run's failure-path 'pending' write
    /// may modify an existing status row but must never resurrect one that evict ("Clear
    /// Index") or mark_orphans deleted mid-run — and the 0-rows-affected return is what tells
    /// `run_full_index` the status vanished mid-run (its cue to evict its own writes and
    /// fail the run). See `set_browse_status_complete_if_live_requires_the_original_mapping`
    /// for the stronger gate the success-path 'complete' write needs instead.
    #[test]
    fn set_browse_status_if_present_never_resurrects_a_deleted_row() {
        let db = test_db();

        // No row → strict no-op, even for the success-path 'complete', and reports 0.
        assert_eq!(
            db.set_browse_status_if_present("repo1", "aaaa111100000000", "complete")
                .unwrap(),
            0
        );
        assert_eq!(count_rows(&db, "browse_cache_status"), 0);

        // Row exists → updated in place, same as set_browse_status, and reports 1.
        db.set_browse_status("repo1", "aaaa111100000000", "in_progress")
            .unwrap();
        assert_eq!(
            db.set_browse_status_if_present("repo1", "aaaa111100000000", "complete")
                .unwrap(),
            1
        );
        assert_eq!(
            db.get_browse_status("repo1").unwrap().get("aaaa111100000000"),
            Some(&"complete".to_string())
        );
    }

    /// Pins `set_browse_status_complete_if_live`'s two-part gate: row-present alone (what
    /// `set_browse_status_if_present` checks) is not enough for the success-path 'complete'
    /// write — it must also confirm `indexed_snapshots` still maps snapshot_id to the *exact*
    /// `snap` id this run's chunks were written against, keyed on `(id, snapshot_id)` together
    /// (not `id` alone) for the same recycled-rowid reason the chunk guard uses.
    #[test]
    fn set_browse_status_complete_if_live_requires_the_original_mapping() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "in_progress")
            .unwrap();
        let snap = db
            .insert_browse_files("aaaa111100000000", &[sample_file_entry()])
            .unwrap();

        // Live mapping, matching id, status row present → succeeds.
        assert_eq!(
            db.set_browse_status_complete_if_live("repo1", "aaaa111100000000", snap)
                .unwrap(),
            1
        );
        assert_eq!(
            db.get_browse_status("repo1").unwrap().get("aaaa111100000000"),
            Some(&"complete".to_string())
        );

        // Reset to 'in_progress' and index a second, distinct snapshot to get a
        // guaranteed-different interned id, then pass *that* id for the first
        // snapshot: the mapping exists, but not under this id → gate fails closed.
        db.set_browse_status("repo1", "aaaa111100000000", "in_progress")
            .unwrap();
        seed_snapshot(&db, "repo1", "bbbb222200000000");
        let other_snap = db
            .insert_browse_files("bbbb222200000000", &[sample_file_entry()])
            .unwrap();
        assert_ne!(snap, other_snap);
        assert_eq!(
            db.set_browse_status_complete_if_live("repo1", "aaaa111100000000", other_snap)
                .unwrap(),
            0,
            "a mismatched snap id must not be accepted as live"
        );
        assert_eq!(
            db.get_browse_status("repo1").unwrap().get("aaaa111100000000"),
            Some(&"in_progress".to_string()),
            "a failed gate must not have modified the status row"
        );

        // The status row itself is gone (e.g. evicted mid-run) → gate fails closed too,
        // even with the correct, still-live snap id.
        db.evict("repo1", "aaaa111100000000").unwrap();
        assert_eq!(
            db.set_browse_status_complete_if_live("repo1", "aaaa111100000000", snap)
                .unwrap(),
            0
        );
    }

    /// Pins the finding this gate exists for: a *different* repo's Clear Index can delete the
    /// shared `browse_cache_files`/`indexed_snapshots` rows while leaving an in-flight run's
    /// own `browse_cache_status` row (still 'in_progress') completely untouched — so a
    /// row-present-only gate (`set_browse_status_if_present`) would wrongly report success.
    /// Repo A already has a 'complete' index of snapshot S; repo B is mid-indexing the same S
    /// (both share the one underlying `browse_cache_files`/`indexed_snapshots` mapping, since
    /// neither table has a repo_id column). The user clears A's index — evict's cross-repo
    /// sharing guard only counts *other* repos' 'complete' status rows, and B's is
    /// 'in_progress', so the shared rows are deleted out from under B's still-running index.
    /// Without `set_browse_status_complete_if_live`, B's terminal write would find its own
    /// status row present, write 'complete', and return success over zero file rows and a
    /// dead mapping — a permanently empty browse/search tree nothing retries or sweeps.
    #[test]
    fn evict_in_another_repo_aborts_an_in_flight_runs_terminal_write() {
        let db = test_db();
        seed_repo(&db, "repo_a");
        seed_repo(&db, "repo_b");
        seed_snapshot(&db, "repo_a", "aaaa111100000000");
        seed_snapshot(&db, "repo_b", "aaaa111100000000");
        db.set_browse_status("repo_a", "aaaa111100000000", "complete")
            .unwrap();
        db.set_browse_status("repo_b", "aaaa111100000000", "in_progress")
            .unwrap();
        // Represents both repos' index runs writing the one shared mapping — repo B's
        // in-flight run captures `snap` here, exactly as `run_full_index` does.
        let snap = db
            .insert_browse_files("aaaa111100000000", &file_entries(3))
            .unwrap();
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);

        // The user clears repo A's index while repo B is still mid-run.
        db.evict("repo_a", "aaaa111100000000").unwrap();

        // Confirms the premise: B's own status row survived untouched (so a row-present-only
        // gate would have let this through), but the shared rows did not.
        assert_eq!(
            db.get_browse_status("repo_b").unwrap().get("aaaa111100000000"),
            Some(&"in_progress".to_string()),
            "evict in another repo must not touch this repo's own status row"
        );
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);

        // Repo B's terminal write must fail closed rather than resurrect 'complete' over
        // zero rows and a dead mapping.
        assert_eq!(
            db.set_browse_status_complete_if_live("repo_b", "aaaa111100000000", snap)
                .unwrap(),
            0,
            "repo B's terminal write must not succeed once its rows were evicted elsewhere"
        );
    }

    /// Pins `set_browse_status_if_listed`: the 'in_progress' entry write of an index run
    /// upserts only while this repo still lists the snapshot, and never creates a status
    /// row for an unlisted one.
    #[test]
    fn set_browse_status_if_listed_gates_on_the_listing_and_upserts() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");

        // Listed → upsert creates the row (this is an upsert, not an UPDATE).
        assert_eq!(
            db.set_browse_status_if_listed("repo1", "aaaa111100000000", "in_progress")
                .unwrap(),
            1
        );
        assert_eq!(
            db.get_browse_status("repo1").unwrap().get("aaaa111100000000"),
            Some(&"in_progress".to_string())
        );

        // Unlisted repo (never listed, or forgotten between queue and start) → no-op, 0.
        assert_eq!(
            db.set_browse_status_if_listed("repo1", "bbbb222200000000", "in_progress")
                .unwrap(),
            0
        );
        assert!(!db
            .get_browse_status("repo1")
            .unwrap()
            .contains_key("bbbb222200000000"));

        // Listing gone after the row existed (forget racing a queued run) → no-op, 0 —
        // a plain set_browse_status here would have flipped has_cleanup_work true solely
        // to later delete the row it just wrote.
        db.remove_snapshot_from_cache("repo1", "aaaa111100000000")
            .unwrap();
        assert_eq!(
            db.set_browse_status_if_listed("repo1", "aaaa111100000000", "pending")
                .unwrap(),
            0
        );
        assert_eq!(
            db.get_browse_status("repo1").unwrap().get("aaaa111100000000"),
            Some(&"in_progress".to_string()),
            "must neither update nor resurrect once the listing is gone"
        );
    }

    // ── has_cleanup_work ─────────────────────────────────────────────────────

    #[test]
    fn has_cleanup_work_false_on_a_clean_db() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();

        assert!(!db.has_cleanup_work().unwrap());
    }

    #[test]
    fn has_cleanup_work_true_for_a_dead_repos_snapshots_cache_row() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        // Simulate a dead repo row without going through remove_repo, which would
        // already clean this up — mirrors mark_orphans's first DELETE clause directly.
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM repositories WHERE id = 'repo1'", [])
            .unwrap();

        assert!(db.has_cleanup_work().unwrap());
    }

    #[test]
    fn has_cleanup_work_true_for_an_unreferenced_indexed_snapshot() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();

        assert!(db.has_cleanup_work().unwrap());
    }

    #[test]
    fn has_cleanup_work_true_for_an_orphaned_browse_cache_status_row() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        // Remove only the snapshots_cache row directly, leaving browse_cache_status
        // behind unmarked — isolates mark_orphans's third DELETE clause.
        db.conn
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM snapshots_cache WHERE snapshot_id = 'aaaa111100000000'",
                [],
            )
            .unwrap();

        assert!(db.has_cleanup_work().unwrap());
    }

    #[test]
    fn has_cleanup_work_true_when_something_is_already_marked_and_awaiting_drain() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();

        // Marking already retired browse_cache_status/repo-keyed rows — the only thing
        // still standing is the orphaned_at stamp itself, awaiting drain_orphans.
        assert!(db.has_cleanup_work().unwrap());
    }

    /// The property that actually matters: `has_cleanup_work` must never say "nothing to
    /// do" while `mark_orphans`/`drain_orphans` would in fact find something, across a
    /// spread of fixture states. This is what catches the probe drifting out of sync with
    /// `mark_orphans` as the source of truth — see `has_cleanup_work`'s doc comment.
    #[test]
    fn has_cleanup_work_agrees_with_mark_and_drain() {
        // Case 1: clean DB — probe false, and running for real confirms nothing found.
        {
            let db = test_db();
            seed_repo(&db, "repo1");
            seed_snapshot(&db, "repo1", "aaaa111100000000");
            db.set_browse_status("repo1", "aaaa111100000000", "complete").unwrap();
            db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();

            assert!(!db.has_cleanup_work().unwrap());
            assert_eq!(db.mark_orphans().unwrap(), 0);
            assert!(orphaned_at_of(&db, "aaaa111100000000").is_none());
            assert_eq!(db.drain_orphans(100).unwrap().rows_deleted, 0);
        }

        // Case 2: a dead-repo orphan exists — probe true, and running for real finds it.
        {
            let db = test_db();
            seed_repo(&db, "repo1");
            seed_snapshot(&db, "repo1", "aaaa111100000000");
            db.insert_browse_files("aaaa111100000000", &[sample_file_entry()]).unwrap();
            db.remove_repo("repo1").unwrap();

            assert!(db.has_cleanup_work().unwrap());
            let marked = db.mark_orphans().unwrap();
            let drained = db.drain_orphans(100).unwrap();
            assert!(marked > 0 || drained.rows_deleted > 0);
        }

        // Case 3: already marked, mid-drain — probe stays true until fully drained.
        {
            let db = test_db();
            seed_repo(&db, "repo1");
            seed_snapshot(&db, "repo1", "aaaa111100000000");
            db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();
            db.remove_repo("repo1").unwrap();
            db.mark_orphans().unwrap();

            assert!(db.has_cleanup_work().unwrap());
            db.drain_orphans(1).unwrap(); // partial drain, one file row left
            assert!(
                db.has_cleanup_work().unwrap(),
                "still-marked row awaiting drain must keep the probe true"
            );
            let batch = db.drain_orphans(100).unwrap();
            assert!(!batch.more_remaining);
            assert!(!db.has_cleanup_work().unwrap());
        }
    }

    #[test]
    fn drain_orphans_respects_max_rows_and_only_retires_the_snapshot_on_the_final_batch() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();
        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();

        let batch1 = db.drain_orphans(2).unwrap();
        assert_eq!(batch1.rows_deleted, 2);
        assert!(batch1.more_remaining);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1, "snapshot has 1 file row left");

        let batch2 = db.drain_orphans(2).unwrap();
        assert_eq!(batch2.rows_deleted, 2, "1 remaining file row + the retired indexed_snapshots row");
        assert!(!batch2.more_remaining);
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
    }

    #[test]
    fn drain_orphans_resumes_after_being_interrupted() {
        let db = test_db();
        seed_repo(&db, "repo1");
        seed_snapshot(&db, "repo1", "aaaa111100000000");
        db.insert_browse_files("aaaa111100000000", &file_entries(5)).unwrap();
        db.remove_repo("repo1").unwrap();
        db.mark_orphans().unwrap();

        // Stop after one batch, as if the button were clicked again later.
        let batch1 = db.drain_orphans(2).unwrap();
        assert!(batch1.more_remaining);

        // Resume: drain to completion.
        let mut total = batch1.rows_deleted;
        loop {
            let batch = db.drain_orphans(2).unwrap();
            total += batch.rows_deleted;
            if !batch.more_remaining {
                break;
            }
        }
        assert_eq!(total, 6, "5 file rows + 1 retired indexed_snapshots row");
        assert_eq!(count_rows(&db, "browse_cache_files"), 0);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 0);
    }

    #[test]
    fn drain_orphans_leaves_a_live_snapshots_file_rows_untouched() {
        let db = test_db();
        seed_repo(&db, "live-repo");
        seed_snapshot(&db, "live-repo", "aaaa111100000000");
        db.set_browse_status("live-repo", "aaaa111100000000", "complete").unwrap();
        db.insert_browse_files("aaaa111100000000", &file_entries(3)).unwrap();

        seed_repo(&db, "dead-repo");
        seed_snapshot(&db, "dead-repo", "bbbb222200000000");
        db.insert_browse_files("bbbb222200000000", &file_entries(3)).unwrap();
        db.remove_repo("dead-repo").unwrap();

        db.mark_orphans().unwrap();
        loop {
            let batch = db.drain_orphans(2).unwrap();
            if !batch.more_remaining {
                break;
            }
        }

        // The live snapshot's index survives a full mark+drain untouched.
        assert_eq!(count_rows(&db, "browse_cache_files"), 3);
        assert_eq!(count_rows(&db, "indexed_snapshots"), 1);
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

        // 5. The new exclude-if-present columns were added by ALTER TABLE (not CREATE
        // TABLE) on this migrated DB — confirm the read path actually works against
        // that shape, since init_schema's `let _ = ...ALTER...` silently swallows
        // failures and every other test builds its schema from CREATE TABLE instead.
        let db = AppDb::new(conn, std::path::PathBuf::new());
        let plans = db.list_backup_plans().unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].name, "Daily");
        assert!(plans[0].exclude_if_present.is_empty());
        assert!(!plans[0].exclude_caches);
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
