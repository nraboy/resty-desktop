use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;

use super::backends;
use super::cache::{AppDb, Credential, FullRepository, MasterKey, PruneHandle, Repository};
use super::crypto;
use super::repo_locks::RepoLocks;
use super::NoConsole;
use crate::tasks::{emit_cancelling, OperationCtx, TaskKind, TaskOrigin, TaskProgress};

/// Input for creating a repository (`add_repo`/`init_repo`). A struct arg rather than
/// six-plus positional parameters — Tauri deserializes command args from a single JSON
/// object either way, so this is free, and it sidesteps `clippy::too_many_arguments`
/// without an `#[allow]`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewRepoInput {
    pub id: String,
    pub name: String,
    pub path: String,
    pub password: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub credentials: Vec<(String, String)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResticStats {
    pub total_size: u64,
    pub total_file_count: u64,
    pub snapshots_count: u64,
    /// Bytes actually stored on disk/remote (post-dedup, post-compression), from a second
    /// `restic stats --mode raw-data --json` call in `fetch_and_cache_stats`. `None` for
    /// legacy cache rows written before this field existed, and whenever the raw-data call
    /// itself fails — that failure is deliberately non-fatal to the refresh as a whole (see
    /// `fetch_and_cache_stats`), so `total_size`/`total_file_count`/`snapshots_count` above
    /// still populate even when this is `None`.
    pub raw_size: Option<u64>,
    /// Unix-seconds timestamp of when this value was cached. `None` only for the
    /// pure `parse_stats_json` path (fresh restic output has no such field) —
    /// callers that hand back a `ResticStats` to the frontend always fill it in
    /// from `repo_stats_cache.cached_at` before returning.
    pub cached_at: Option<i64>,
}

/// Finds the last non-blank line in restic `--json` stdout. restic sometimes emits
/// blank/whitespace-only trailing lines; the real JSON payload is always the last
/// non-blank one. Shared by `parse_stats_json` here and `get_snapshot_stats` in
/// snapshot.rs, both of which parse `restic stats --json` output.
pub(crate) fn last_nonblank_line(stdout: &str) -> Option<&str> {
    stdout.lines().rfind(|l| !l.trim().is_empty())
}

/// Parses `restic stats --json` stdout into `ResticStats`. Pure — no restic call, no
/// DB write — so `fetch_and_cache_stats` (the async command wrapper) can be tested by
/// feeding it captured stdout instead of shelling out to a real restic binary.
pub(crate) fn parse_stats_json(stdout: &str) -> Result<ResticStats, String> {
    let last_line = last_nonblank_line(stdout).ok_or_else(|| "No output from restic stats".to_string())?;
    let v: serde_json::Value = serde_json::from_str(last_line).map_err(|e| e.to_string())?;
    Ok(ResticStats {
        total_size: v["total_size"].as_u64().unwrap_or(0),
        total_file_count: v["total_file_count"].as_u64().unwrap_or(0),
        snapshots_count: v["snapshots_count"].as_u64().unwrap_or(0),
        raw_size: None,
        cached_at: None,
    })
}

/// Parses `restic stats --mode raw-data --json` stdout into the on-disk stored-size figure
/// (post-dedup, post-compression). Pure, mirroring `parse_stats_json` — same last-nonblank-line
/// handling, same tolerance for restic emitting other NDJSON lines before the summary. Kept
/// separate from `parse_stats_json` rather than folded in behind a mode flag: that function is
/// shared with `get_snapshot_stats` in snapshot.rs, which never calls raw-data mode.
pub(crate) fn parse_raw_data_size(stdout: &str) -> Result<u64, String> {
    let last_line = last_nonblank_line(stdout).ok_or_else(|| "No output from restic stats".to_string())?;
    let v: serde_json::Value = serde_json::from_str(last_line).map_err(|e| e.to_string())?;
    Ok(v["total_size"].as_u64().unwrap_or(0))
}

/// Rejects an empty password for repo *creation* (`init_repo`). Passwordless
/// repos may be opened/imported/exported, but not created from this app. This is
/// a defense-in-depth guard behind the UI (the Init modal already requires a
/// password); extracted as a helper so it can be unit-tested without Tauri state.
/// Uses `is_empty()` to match the codebase's empty-string-means-passwordless
/// convention (see `apply_repo_password`).
pub(crate) fn validate_init_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("A password is required to create a repository.".to_string());
    }
    Ok(())
}

/// Validates a user-supplied restic binary path (already trimmed). Pure — no
/// filesystem I/O beyond checking existence of an already-resolved absolute path, no
/// DB write — so `set_restic_path` can be tested without a `tauri::State<AppDb>`.
pub(crate) fn validate_restic_path(trimmed: &str) -> Result<(), String> {
    if trimmed.is_empty() {
        return Err("Restic path must not be empty".to_string());
    }
    // If the value looks like an absolute path, verify the file exists.
    if (trimmed.starts_with('/') || trimmed.starts_with('\\') || trimmed.contains(":\\"))
        && !std::path::Path::new(trimmed).is_file() {
            return Err(format!("No file found at '{trimmed}'"));
        }
    Ok(())
}

/// Applies a repo's password to a restic `Command`: a normal password sets
/// `RESTIC_PASSWORD`; an empty stored password means a repo created with
/// `restic init --insecure-no-password`, which restic requires the caller to pass
/// `--insecure-no-password` on every subsequent command (an empty/unset
/// `RESTIC_PASSWORD` alone makes restic prompt interactively, not use no password).
/// Setting both the flag and the env var is a restic error, so the two are mutually
/// exclusive.
pub(crate) fn apply_repo_password(cmd: &mut std::process::Command, password: &str) {
    if password.is_empty() {
        cmd.arg("--insecure-no-password");
    } else {
        cmd.env("RESTIC_PASSWORD", password);
    }
}

/// Same as `apply_repo_password` but for a copy/mirror source repo's `--from-*` flags.
pub(crate) fn apply_from_repo_password(cmd: &mut std::process::Command, password: &str) {
    if password.is_empty() {
        cmd.arg("--from-insecure-no-password");
    } else {
        cmd.env("RESTIC_FROM_PASSWORD", password);
    }
}

/// Applies a repo's password (`apply_repo_password`) and, when the repo is marked
/// read-only, restic's own `--no-lock` — the flag that lets restic operate against a
/// repository whose backing filesystem/mount is genuinely read-only, by skipping the
/// lock file it would otherwise try to write. `--no-lock` is a global restic flag, so
/// this is the one place every read-type call needs to touch; write-type calls are
/// refused before they ever reach here (see `ensure_writable`).
pub(crate) fn apply_repo_flags(cmd: &mut std::process::Command, repo: &FullRepository) {
    apply_backend_env(cmd, &repo.credentials);
    apply_repo_password(cmd, &repo.password);
    if repo.read_only {
        cmd.arg("--no-lock");
    }
}

/// Same as `apply_repo_flags` but for a copy/mirror source repo's `--from-*` flags —
/// used when the *source* of a copy/mirror is read-only. The destination is never
/// read-only (`ensure_writable` refuses it earlier), so it has no `--no-lock` counterpart.
pub(crate) fn apply_from_repo_flags(cmd: &mut std::process::Command, repo: &FullRepository) {
    apply_backend_env(cmd, &repo.credentials);
    apply_from_repo_password(cmd, &repo.password);
    if repo.read_only {
        cmd.arg("--no-lock");
    }
}

/// Sets each stored backend credential (e.g. `AWS_ACCESS_KEY_ID`, `B2_ACCOUNT_ID`) as
/// an env var on `cmd`. A repo with no stored credentials (the ambient default every
/// pre-existing repo is in) sets nothing at all here, so restic's own credential chain
/// — inherited process env, `~/.aws/credentials`, an IAM role — is left exactly as it
/// was. A credential with an empty value is skipped rather than set as `""`: an empty
/// `AWS_ACCESS_KEY_ID=""` would *break* that chain rather than falling through to it.
///
/// A reserved key (`backends::is_reserved_key` — `PATH` or any `RESTIC_*` var) is
/// skipped unconditionally, regardless of call order. Callers set `RESTIC_REPOSITORY`
/// (and `RESTIC_FROM_REPOSITORY`, `RESTIC_COMPRESSION`) on `cmd` *before* calling this,
/// so without this guard a stored credential of one of those names would win the
/// collision and silently redirect the operation to a different repository.
/// `validate_credentials` (`backends.rs`) already rejects a reserved key at entry, but
/// that only covers rows that went through it — an imported bundle (or a hand-edited
/// database) is a second path a row can reach the DB by, so this is the guarantee that
/// does not depend on every ingest path remembering to validate.
pub(crate) fn apply_backend_env(cmd: &mut std::process::Command, credentials: &[Credential]) {
    for c in credentials {
        if backends::is_reserved_key(&c.key) {
            continue;
        }
        if !c.value.is_empty() {
            cmd.env(&c.key, &c.value);
        }
    }
}

/// Merges a copy/mirror destination's and source's stored backend credentials into
/// one set to apply to a single restic process. restic has no `--from-`-side
/// counterpart for backend credentials (unlike the password, which has
/// `RESTIC_FROM_PASSWORD`) — they're both applied as plain process env — so two repos
/// that need *different* values for the same key (e.g. two B2 accounts) genuinely
/// cannot be used together in one `copy`/`mirror` run. A shared key with an *equal*
/// value (e.g. the same B2 account backing two buckets) is fine and merges cleanly.
pub(crate) fn merge_credentials(
    dest: &[Credential],
    src: &[Credential],
) -> Result<Vec<Credential>, String> {
    let mut merged: Vec<Credential> = dest.to_vec();
    for s in src {
        match merged.iter().find(|d| d.key == s.key) {
            Some(existing) if existing.value == s.value => {}
            Some(_) => {
                return Err(format!(
                    "The source and destination repositories both set '{}' to a different \
                     value. restic applies backend credentials as process environment \
                     variables, so two repositories on the same backend with conflicting \
                     credentials for the same key can't be used together in one copy/mirror run.",
                    s.key
                ));
            }
            None => merged.push(s.clone()),
        }
    }
    Ok(merged)
}

/// Runs `restic unlock` for `repo` and discards the result — the cancel-path recovery
/// mechanism used after a SIGKILL (see CLAUDE.md's Restic Integration section). Takes
/// a full, already-resolved `FullRepository` (clone it, don't rebuild one field by
/// field) so backend credentials and read-only status are never silently dropped the
/// way a hand-built `FullRepository { .. }` literal could drop a field added after it
/// was written. A read-only repo never held a lock (opened with `--no-lock`), so its
/// unlock would only fail against a genuinely read-only backing store — callers should
/// skip a read-only repo before calling this, matching the existing convention.
pub(crate) fn unlock_quietly(repo: &FullRepository, restic_path: &str) {
    let _ = run_restic_with_path(repo, vec!["unlock"], restic_path);
}

/// Error returned by every write-type command when the target repository is marked
/// read-only. Read-type operations (browse, restore, search, stats, check, diff, and
/// being a copy/mirror *source*) are unaffected — see `apply_repo_flags`.
pub(crate) const READ_ONLY_REPO_ERROR: &str =
    "This repository is marked read-only; writing operations are disabled.";

/// Guards every write-type command (backup, forget/retention, prune, tag, delete,
/// unlock, and being a copy/mirror *destination*). Call immediately after resolving
/// the `FullRepository`, before any restic process is spawned.
pub(crate) fn ensure_writable(repo: &FullRepository) -> Result<(), String> {
    if repo.read_only {
        return Err(READ_ONLY_REPO_ERROR.to_string());
    }
    Ok(())
}

pub fn run_restic_with_path(
    repo: &FullRepository,
    args: Vec<&str>,
    restic_path: &str,
) -> Result<String, String> {
    let mut cmd = std::process::Command::new(restic_path);
    cmd.args(args).env("RESTIC_REPOSITORY", &repo.path);
    apply_repo_flags(&mut cmd, repo);
    let output = cmd
        .stdin(std::process::Stdio::null())
        .no_console()
        .augment_path()
        .output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| format!("restic output contained invalid UTF-8: {e}"))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// One-shot restic on a blocking-pool thread so it never occupies an async-runtime
/// worker. Owns its inputs so they can cross the spawn_blocking boundary.
pub(crate) async fn run_restic_blocking(
    repo: FullRepository,
    args: Vec<String>,
    restic_path: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_restic_with_path(&repo, arg_refs, &restic_path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_repos(db: State<'_, AppDb>) -> Result<Vec<Repository>, String> {
    db.list_repos()
}

#[tauri::command]
pub async fn add_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    input: NewRepoInput,
) -> Result<(), String> {
    let kind = backends::detect_kind(&input.path);
    backends::validate_credentials(kind, &input.credentials)?;
    let key = master_key.get()?;
    let (nonce, ciphertext) = crypto::encrypt(&key, input.password.as_bytes())?;
    let credentials: Vec<Credential> = input
        .credentials
        .into_iter()
        .map(|(key, value)| Credential { key, value })
        .collect();
    let (cred_nonce, cred_ciphertext) = super::cache::encode_credentials(&key, &credentials)?;
    db.add_repo(
        &input.id,
        &input.name,
        &input.path,
        &nonce,
        &ciphertext,
        input.read_only,
        cred_nonce.as_deref(),
        cred_ciphertext.as_deref(),
    )
}

#[tauri::command]
pub async fn remove_repo(db: State<'_, AppDb>, repo_id: String) -> Result<(), String> {
    db.remove_repo(&repo_id)
}

#[tauri::command]
pub async fn rename_repo(
    db: State<'_, AppDb>,
    repo_id: String,
    new_name: String,
) -> Result<(), String> {
    db.rename_repo(&repo_id, &new_name)
}

#[tauri::command]
pub async fn update_repo_path(
    db: State<'_, AppDb>,
    repo_id: String,
    new_path: String,
) -> Result<(), String> {
    db.update_repo_path(&repo_id, &new_path)
}

#[tauri::command]
pub async fn update_repo_read_only(
    db: State<'_, AppDb>,
    repo_id: String,
    read_only: bool,
) -> Result<(), String> {
    db.set_repo_read_only(&repo_id, read_only)
}

#[tauri::command]
pub async fn get_repo_password(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<String, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    Ok(repo.password.clone())
}

/// Returns each stored credential's key and value — see `AppDb::get_repo_credentials`'s
/// doc comment for why this matches `get_repo_password`'s threat model rather than the
/// earlier keys-only design.
#[tauri::command]
pub async fn get_repo_credentials(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
) -> Result<Vec<(String, String)>, String> {
    let key = master_key.get()?;
    db.get_repo_credentials(&repo_id, &key)
}

/// Updates a repo's password and/or stored backend credentials in one transaction.
/// `password`/`credentials` of `None` leaves that field unchanged (the edit modal's
/// "leave blank to keep current" behavior); `Some(vec![])` for credentials clears
/// them back to ambient mode. Validated against the path's derived backend kind
/// before writing.
#[tauri::command]
pub async fn update_repo_secrets(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_id: String,
    password: Option<String>,
    credentials: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    let key = master_key.get()?;
    if let Some(creds) = &credentials {
        let repos = db.list_repos()?;
        let path = repos
            .iter()
            .find(|r| r.id == repo_id)
            .map(|r| r.path.clone())
            .ok_or_else(|| "Repository not found".to_string())?;
        backends::validate_credentials(backends::detect_kind(&path), creds)?;
    }
    let credentials = credentials.map(|creds| {
        creds.into_iter().map(|(key, value)| Credential { key, value }).collect()
    });
    db.update_repo_secrets(&repo_id, &key, password, credentials)
}

/// Initialise a new restic repository, then persist it.
#[tauri::command]
pub async fn init_repo(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    input: NewRepoInput,
) -> Result<(), String> {
    validate_init_password(&input.password)?;
    let kind = backends::detect_kind(&input.path);
    backends::validate_credentials(kind, &input.credentials)?;
    let credentials: Vec<Credential> = input
        .credentials
        .into_iter()
        .map(|(key, value)| Credential { key, value })
        .collect();
    let restic_path = super::get_restic_path(&db);
    // `restic init` always creates a writable repo — read_only is always false here.
    let dummy = FullRepository {
        path: input.path.clone(),
        password: input.password.clone(),
        read_only: false,
        credentials: credentials.clone(),
    };
    run_restic_blocking(dummy, vec!["init".into()], restic_path).await.map(|_| ())?;

    let key = master_key.get()?;
    let (nonce, ciphertext) = crypto::encrypt(&key, input.password.as_bytes())?;
    let (cred_nonce, cred_ciphertext) = super::cache::encode_credentials(&key, &credentials)?;
    db.add_repo(
        &input.id,
        &input.name,
        &input.path,
        &nonce,
        &ciphertext,
        false,
        cred_nonce.as_deref(),
        cred_ciphertext.as_deref(),
    )
}

/// Test an unsaved repo connection (used by the "Test Connection" button in the add modal).
#[tauri::command]
pub async fn test_repo_connection(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    path: String,
    password: String,
    read_only: bool,
    credentials: Vec<(String, String)>,
) -> Result<(), String> {
    let kind = backends::detect_kind(&path);
    backends::validate_credentials(kind, &credentials)?;
    let restic_path = super::get_restic_path(&db);
    // No saved repoId yet (the repo isn't added until the test passes) — matches
    // prune_all_repos' empty-repoId convention for the same "no single id" case.
    let task_ctx = OperationCtx::new(app, TaskKind::TestConnection, String::new(), None, TaskOrigin::Manual, None);
    let credentials: Vec<Credential> =
        credentials.into_iter().map(|(key, value)| Credential { key, value }).collect();
    let dummy = FullRepository { path, password, read_only, credentials };
    let result = run_restic_blocking(dummy, vec!["snapshots".into(), "--json".into()], restic_path)
        .await
        .map(|_| ());
    match &result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

/// Cache-only read: never shells out to restic, even on a miss. This used to fall through to
/// `fetch_and_cache_stats` on a miss — fine under normal operation (a genuine miss only ever
/// happened for a repo that had literally never been fetched), but "Clear All Cache"
/// (`AppDb::clear_cache`, SettingsPage) wipes `repo_stats_cache` for every repo at once, and
/// RepositoriesPage calls this command for every repo on mount (see CLAUDE.md's Intentional
/// Designs). Together, that meant the very next visit to Repositories after a cache clear
/// silently kicked off a real `restic stats` subprocess for every single repo — a manual-only
/// feature auto-refreshing itself the moment its cache was cleared, contradicting the whole
/// point of the manual-only redesign (see this file's `refresh_repo_stats` doc comment).
/// Returns `Err` on a miss so the frontend's existing "couldn't load" fallback (the `—`
/// placeholder) applies — same as any other failed fetch; the user must click Refresh (row or
/// All) to actually populate it, exactly like a brand-new repo always required. No restic call,
/// no `RepoLocks`/`MasterKey` needed, so this is a plain sync command (matches `list_repos`).
#[tauri::command]
pub fn get_repo_stats(db: State<'_, AppDb>, repo_id: String) -> Result<ResticStats, String> {
    match db.get_stats(&repo_id) {
        Ok(Some(stats)) => Ok(stats),
        Ok(None) => Err("No cached stats for this repository".to_string()),
        Err(e) => Err(e),
    }
}

/// Manual-only refresh: stats are never auto-evicted (see CLAUDE.md's Restic
/// Integration section) — this is the sole way a repo's cached stats change,
/// aside from the very first fetch. A failed refresh leaves the last-good
/// cached value (and its `cached_at`) untouched, since `set_stats` only
/// overwrites on a successful fetch below.
#[tauri::command]
pub async fn refresh_repo_stats(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<ResticStats, String> {
    fetch_and_cache_stats(&app, &db, &master_key, &repo_locks, &repo_id).await
}

/// `task_ctx` is created *first*, before any fallible step, and every step below reports
/// through it explicitly (`task_ctx.failed(e)`) rather than via `?` — so every way this can
/// fail (locked app, deleted repo, the restic call itself, a malformed response, the cache
/// write) reliably emits a `task` "failed" event. This matters because the frontend now
/// derives a boolean "last refresh failed" marker purely from the bus (see `activity.tsx`'s
/// `reduceStatsOps`) with no fallback to the invoke promise's own rejection — relying on `?`
/// here would let some failures fall through to `OperationCtx`'s `Drop` backstop (or, for the
/// two steps before `task_ctx` used to exist, emit nothing at all), silently leaving that
/// marker unset even though the refresh genuinely failed.
async fn fetch_and_cache_stats(
    app: &tauri::AppHandle,
    db: &AppDb,
    master_key: &MasterKey,
    repo_locks: &RepoLocks,
    repo_id: &str,
) -> Result<ResticStats, String> {
    let task_ctx = OperationCtx::new(app.clone(), TaskKind::Stats, repo_id, None, TaskOrigin::Manual, None);

    let key = match master_key.get() {
        Ok(k) => k,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let repo = match db.get_full_repo(repo_id, &key) {
        Ok(r) => r,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let restic_path = super::get_restic_path(db);
    let _rg = repo_locks.read(&repo.path);
    // Cloning the whole `FullRepository` (never rebuilding field-by-field) so the second
    // call below keeps any stored backend credentials — see docs/restic.md's
    // `apply_backend_env` note on why every multi-call site does this.
    let result = run_restic_blocking(repo.clone(), vec!["stats".into(), "--json".into()], restic_path.clone()).await;
    let stdout = match result {
        Ok(stdout) => stdout,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    let mut stats = match parse_stats_json(&stdout) {
        Ok(s) => s,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    // Second call, on-disk stored size (post-dedup, post-compression). Deliberately
    // non-fatal: a failure here (e.g. an older restic without raw-data support, or a
    // transient remote-backend hiccup) must not turn an otherwise-successful refresh into
    // "refresh failed" and blank out the repo's last-good restore-size numbers — it just
    // leaves `raw_size` unset for this cycle. See docs/restic.md and docs/decisions.md.
    stats.raw_size = match run_restic_blocking(
        repo,
        vec!["stats".into(), "--mode".into(), "raw-data".into(), "--json".into()],
        restic_path,
    )
    .await
    {
        Ok(stdout) => parse_raw_data_size(&stdout).ok(),
        Err(_) => None,
    };
    // Cache write happens before `finished()` is emitted, on purpose: a `task`-bus
    // consumer that hears "finished" and re-reads `get_repo_stats` must never race
    // ahead of this write. See CLAUDE.md's Operation Event Bus section.
    let ts = match db.set_stats(repo_id, &stats) {
        Ok(t) => t,
        Err(e) => {
            task_ctx.failed(e.clone());
            return Err(e);
        }
    };
    stats.cached_at = Some(ts);
    task_ctx.finished();
    Ok(stats)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CheckResult {
    pub success: bool,
    pub errors: Vec<String>,
    pub duration_seconds: f64,
}

#[tauri::command]
pub async fn check_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<CheckResult, String> {
    let key = master_key.get()?;
    let repo = db.get_full_repo(&repo_id, &key)?;
    let restic_path = super::get_restic_path(&db);
    let task_ctx = OperationCtx::new(app, TaskKind::Check, repo_id, None, TaskOrigin::Manual, None);

    // `check` is a shared-lock read — register as a reader, held across the
    // spawn_blocking below for the whole child-process lifetime.
    let _rg = repo_locks.read(&repo.path);

    let spawn_result = tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let mut cmd = std::process::Command::new(&restic_path);
        cmd.args(["check", "--json"]).env("RESTIC_REPOSITORY", &repo.path);
        apply_repo_flags(&mut cmd, &repo);
        let output = cmd
            .stdin(std::process::Stdio::null())
            .no_console()
            .augment_path()
            .output()
            .map_err(|e| format!("Failed to run restic: {e}"))?;
        Ok::<_, String>((output, started.elapsed().as_secs_f64()))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    // A `finished` task means the check *ran*; the pass/fail verdict is part of
    // CheckResult's data, not the task's own outcome. Only a spawn/process failure
    // (couldn't even run restic) is a task-level `failed`.
    match &spawn_result {
        Ok(_) => task_ctx.finished(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    let (output, duration_seconds) = spawn_result?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut errors: Vec<String> = Vec::new();

    for line in stdout.lines().chain(stderr.lines()) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            let msg = match v["message_type"].as_str() {
                Some("error") => v["error"]["message"].as_str().map(str::to_string),
                Some("exit_error") => v["message"].as_str().map(str::to_string),
                _ => None,
            };
            if let Some(m) = msg {
                if !errors.contains(&m) {
                    errors.push(m);
                }
            }
        }
    }

    if !output.status.success() && errors.is_empty() {
        let raw = stderr.trim().to_string();
        if !raw.is_empty() {
            errors.push(raw);
        }
    }

    Ok(CheckResult {
        success: output.status.success(),
        errors,
        duration_seconds,
    })
}

#[tauri::command]
pub fn get_restic_path(db: State<'_, AppDb>) -> Result<String, String> {
    db.get_setting("restic_path", "restic")
}

#[tauri::command]
pub fn set_restic_path(db: State<'_, AppDb>, path: String) -> Result<(), String> {
    let trimmed = path.trim();
    validate_restic_path(trimmed)?;
    db.set_setting("restic_path", trimmed)
}

#[tauri::command]
pub fn get_compression(db: State<'_, AppDb>) -> Result<String, String> {
    db.get_setting("compression", "auto")
}

#[tauri::command]
pub fn set_compression(db: State<'_, AppDb>, value: String) -> Result<(), String> {
    db.set_setting("compression", &value)
}

#[tauri::command]
pub fn get_restore_path(app: tauri::AppHandle, db: State<'_, AppDb>) -> Result<String, String> {
    let stored = db.get_setting("restore_path", "")?;
    if !stored.is_empty() {
        return Ok(stored);
    }
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("Could not determine home directory: {e}"))?;
    let default_path = home.join("restores").to_string_lossy().into_owned();
    db.set_setting("restore_path", &default_path)?;
    Ok(default_path)
}

#[tauri::command]
pub fn set_restore_path(db: State<'_, AppDb>, path: String) -> Result<(), String> {
    db.set_setting("restore_path", path.trim())
}

#[tauri::command]
pub fn get_restic_version(db: State<'_, AppDb>) -> Result<String, String> {
    let restic_path = super::get_restic_path(&db);
    let output = std::process::Command::new(&restic_path)
        .arg("version")
        .stdin(std::process::Stdio::null())
        .no_console()
        .augment_path()
        .output()
        .map_err(|_| format!("restic not found at '{restic_path}'"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Outcome of one `restic prune` attempt (see `run_one_prune_attempt`).
enum PruneAttempt {
    Success,
    Cancelled,
    Failed(String),
}

/// One spawn-poll-capture attempt of `restic prune` against `full`, respecting
/// `prune_handle.cancelled` throughout via responsive `try_wait` polling — factored out here so
/// both call sites, and the retry-on-"already locked" loop each wraps around it, share one
/// implementation instead of duplicating this logic twice. Captures stderr (previously
/// discarded via `Stdio::null()`) so a failure can actually be inspected for "already locked"
/// — and, as a side benefit, callers now get restic's real error text instead of a generic
/// "Prune failed".
async fn run_one_prune_attempt(
    restic_path: &str,
    full: &FullRepository,
    prune_handle: &PruneHandle,
) -> Result<PruneAttempt, String> {
    use std::io::{BufReader, Read};

    let mut cmd = std::process::Command::new(restic_path);
    cmd.arg("prune").env("RESTIC_REPOSITORY", &full.path);
    apply_backend_env(&mut cmd, &full.credentials);
    apply_repo_password(&mut cmd, &full.password);
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .no_console()
        .augment_path()
        .spawn()
        .map_err(|e| format!("Failed to run restic: {e}"))?;

    let stderr = child.stderr.take().ok_or("failed to capture restic stderr")?;
    let stderr_thread = std::thread::spawn(move || {
        let mut s = String::new();
        BufReader::new(stderr).read_to_string(&mut s).ok();
        s
    });

    {
        let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
        *guard = Some(child);
    }

    let status = loop {
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            // A concurrent cancel_prune's own kill() may have raced the child being stored
            // above and seen `None` in the guard (no-op'd) if cancellation landed between
            // spawn() returning and this function's own store completing. Without our own kill
            // attempt here, the guard-clear right below would silently DROP a still-live
            // std::process::Child — Child::drop() does not kill the OS process — orphaning a
            // `restic prune` that keeps running (and keeps holding the repo's exclusive lock)
            // while the UI already reports this as cancelled. Killing an already-exited/already
            // -killed child is a harmless no-op, so it's safe to always attempt this here.
            if let Ok(mut guard) = prune_handle.child.lock() {
                if let Some(ref mut c) = *guard {
                    let _ = c.kill();
                }
            }
            // kill() only sends the signal — it doesn't wait for the OS to tear the process
            // down, and dropping a `Child` handle (the guard-clear right below) does NOT reap
            // it either. Without this, the killed process lingers as a zombie that Rust never
            // waits on again. While it's a zombie its PID is still "alive" as far as a
            // same-host liveness check is concerned, which can fool restic's own stale-lock
            // detection into believing the lock's owning process is still running — so the
            // `unlock` call below silently no-ops and the repo stays exclusively locked for
            // the next prune attempt. Poll (non-blockingly) until the process is actually
            // reaped before proceeding.
            loop {
                let reaped = {
                    let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
                    match *guard {
                        Some(ref mut c) => c.try_wait().map_err(|e| e.to_string())?.is_some(),
                        None => true,
                    }
                };
                if reaped {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            break None;
        }
        let maybe_status = {
            let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
            if let Some(ref mut c) = *guard {
                c.try_wait().map_err(|e| e.to_string())?
            } else {
                break None;
            }
        };
        if let Some(s) = maybe_status {
            break Some(s);
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    };

    {
        let mut guard = prune_handle.child.lock().map_err(|e| e.to_string())?;
        *guard = None;
    }

    let captured_stderr = stderr_thread.join().unwrap_or_default();

    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
        // A read-only repo never reaches run_one_prune_attempt (ensure_writable refuses
        // prune_repo/prune_all_repos first), so no lock was ever taken — no unlock needed.
        // Clone the whole repo (not a hand-rebuilt literal) so backend credentials ride
        // along — see `unlock_quietly`'s doc comment on why that matters.
        let _ = run_restic_blocking(full.clone(), vec!["unlock".to_string()], restic_path.to_string()).await;
        return Ok(PruneAttempt::Cancelled);
    }

    let status = status.ok_or_else(|| "Prune ended unexpectedly".to_string())?;
    if status.success() {
        Ok(PruneAttempt::Success)
    } else {
        let msg = captured_stderr.trim();
        Ok(PruneAttempt::Failed(if msg.is_empty() { "Prune failed".to_string() } else { msg.to_string() }))
    }
}

#[tauri::command]
pub async fn prune_all_repos(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    prune_handle: State<'_, PruneHandle>,
    repo_locks: State<'_, RepoLocks>,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Serializes prune_repo/prune_all_repos — they previously shared this handle with no
    // serialization, so a concurrent second run could clobber the first run's
    // `child`/`cancelled` state (a second Stop could kill the wrong process, or vice versa).
    if prune_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A prune is already in progress".to_string());
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&prune_handle.busy);

    prune_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);

    let task_ctx = OperationCtx::new(
        app.clone(),
        TaskKind::Prune,
        // No single repoId for a multi-repo prune — left empty, matching the
        // "done" prune:progress event's existing empty-repoId convention.
        String::new(),
        None,
        TaskOrigin::Manual,
        Some(prune_handle.current_task.clone()),
    );
    let task_progress = task_ctx.progress_emitter();

    // Everything fallible below is captured into `result` (via `break 'body` /
    // explicit match instead of `?`/`return`) rather than exiting the fn directly,
    // so the task_ctx terminal call below always runs exactly once, matching the
    // right phase (Finished/Cancelled/Failed) for every exit path.
    let result: Result<(), String> = 'body: {
        let key = match master_key.get() {
            Ok(k) => k,
            Err(e) => break 'body Err(e),
        };
        // Read-only repos are skipped rather than failing the whole batch — prune is a
        // write, so there's nothing to prune on a repo that can't be written to.
        let repos: Vec<Repository> = match db.list_repos() {
            Ok(r) => r.into_iter().filter(|r| !r.read_only).collect(),
            Err(e) => break 'body Err(e),
        };
        let total = repos.len();
        let restic_path = super::get_restic_path(&db);

        for (i, repo) in repos.iter().enumerate() {
            if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                break 'body Err("Cancelled".to_string());
            }

            task_progress.emit(TaskProgress {
                items_done: Some(i as u64),
                items_total: Some(total as u64),
                label: Some(repo.name.clone()),
                repo_id: Some(repo.id.clone()),
                ..Default::default()
            });

            let full = match db.get_full_repo(&repo.id, &key) {
                Ok(r) => r,
                Err(e) => break 'body Err(e),
            };

            // `prune` takes restic's exclusive lock — wait for this repo to go idle first
            // (see CLAUDE.md's Concurrency section / repo_locks.rs). Scoped to this loop
            // iteration: dropped (releasing the exclusive claim) before the next repo.
            let _wg = repo_locks.write(&full.path).await;

            // The wait above can take a while if the repo was genuinely busy — re-check
            // cancellation before spawning, otherwise a Stop click during the wait can't be
            // caught by cancel_prune (child was still None) and this repo's restic process
            // would be orphaned, unkillable, running to completion in the background.
            if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                break 'body Err("Cancelled".to_string());
            }

            // Retry on a genuine EXTERNAL lock collision (a different machine/tool's restic
            // process — RepoLocks above only coordinates this app's own operations).
            let mut outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                Ok(o) => o,
                Err(e) => break 'body Err(e),
            };
            for _ in 0..2 {
                match &outcome {
                    PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                            break 'body Err("Cancelled".to_string());
                        }
                        outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                            Ok(o) => o,
                            Err(e) => break 'body Err(e),
                        };
                    }
                    _ => break,
                }
            }

            match outcome {
                PruneAttempt::Success => {}
                PruneAttempt::Cancelled => break 'body Err("Cancelled".to_string()),
                PruneAttempt::Failed(msg) => break 'body Err(format!("Prune failed for '{}': {}", repo.name, msg)),
            }

            // Post-work emit so the final iteration reports `total of total` rather than
            // `total - 1 of total` — the pre-work emit above only ever reports the index of
            // the repo currently starting, never the one that just finished. Matches
            // index_snapshots_batch's (browse.rs) already-correct pattern.
            task_progress.emit(TaskProgress {
                items_done: Some(i as u64 + 1),
                items_total: Some(total as u64),
                label: Some(repo.name.clone()),
                repo_id: Some(repo.id.clone()),
                ..Default::default()
            });
        }

        Ok(())
    };

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[tauri::command]
pub async fn prune_repo(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    prune_handle: State<'_, PruneHandle>,
    repo_locks: State<'_, RepoLocks>,
    repo_id: String,
) -> Result<(), String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    if prune_handle
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("A prune is already in progress".to_string());
    }
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&prune_handle.busy);

    prune_handle.cancelled.store(false, std::sync::atomic::Ordering::SeqCst);

    let task_ctx = OperationCtx::new(
        app,
        TaskKind::Prune,
        repo_id.clone(),
        None,
        TaskOrigin::Manual,
        Some(prune_handle.current_task.clone()),
    );

    let result: Result<(), String> = 'body: {
        let key = match master_key.get() {
            Ok(k) => k,
            Err(e) => break 'body Err(e),
        };
        let full = match db.get_full_repo(&repo_id, &key) {
            Ok(r) => r,
            Err(e) => break 'body Err(e),
        };
        if let Err(e) = ensure_writable(&full) {
            break 'body Err(e);
        }
        let restic_path = super::get_restic_path(&db);

        // `prune` takes restic's exclusive lock — wait for the repo to go idle first (see
        // CLAUDE.md's Concurrency section / repo_locks.rs).
        let _wg = repo_locks.write(&full.path).await;

        // The wait above can take a while if the repo was genuinely busy. If Stop was clicked
        // during that wait, prune_handle.child was still None at that moment, so cancel_prune's
        // kill() was a no-op — bail out now, before spawning, so we never orphan an unkillable
        // restic process. `_wg` and `_busy` both drop automatically on this early return,
        // releasing the exclusive claim and the busy flag exactly like every other early-return
        // path in this function already does.
        if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
            break 'body Err("Cancelled".to_string());
        }

        // Run the prune, retrying up to 2 additional times if it collides with a *different*
        // process's or machine's genuine restic lock — RepoLocks above only coordinates this app's
        // own operations, not an external restic/Backrest/other-computer process (see CLAUDE.md's
        // Concurrency section). Matches apply_retention's retry pattern.
        let mut outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
            Ok(o) => o,
            Err(e) => break 'body Err(e),
        };
        for _ in 0..2 {
            match &outcome {
                PruneAttempt::Failed(msg) if msg.contains("already locked") => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    // A cancel during the inter-retry sleep must stop us from spawning another
                    // attempt — same reasoning as the cancellation check above.
                    if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                        break 'body Err("Cancelled".to_string());
                    }
                    outcome = match run_one_prune_attempt(&restic_path, &full, &prune_handle).await {
                        Ok(o) => o,
                        Err(e) => break 'body Err(e),
                    };
                }
                _ => break,
            }
        }

        match outcome {
            PruneAttempt::Success => Ok(()),
            PruneAttempt::Cancelled => Err("Cancelled".to_string()),
            PruneAttempt::Failed(msg) => Err(msg),
        }
    };

    match &result {
        Ok(_) => task_ctx.finished(),
        Err(_) if prune_handle.cancelled.load(std::sync::atomic::Ordering::SeqCst) => task_ctx.cancelled(),
        Err(e) => task_ctx.failed(e.clone()),
    }
    result
}

#[tauri::command]
pub fn get_tray_enabled(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("tray_enabled", "false")? == "true")
}

#[tauri::command]
pub fn set_tray_enabled(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("tray_enabled", if value { "true" } else { "false" })
}

#[tauri::command]
pub fn get_tray_warning() -> &'static str {
    #[cfg(target_os = "linux")]
    return "System tray support on Linux depends on your desktop environment. It works on KDE and XFCE, but GNOME requires the AppIndicator extension. If the tray icon does not appear after enabling, the app will continue running as a background process — relaunch it to restore the window.";
    #[cfg(not(target_os = "linux"))]
    return "";
}

/// Launch-at-login state, deliberately *not* backed by an `app_settings` row like every
/// other toggle in this file. `tauri-plugin-autostart` reads the real OS entry (macOS
/// LaunchAgent plist / Windows HKCU Run value / Linux XDG .desktop), so this can never
/// drift from what the OS will actually do — including when the user removes the entry
/// outside the app. A mirrored DB row would need reconciliation on every read.
#[tauri::command]
pub fn get_launch_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_launch_at_login(app: tauri::AppHandle, value: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    if value {
        return manager.enable().map_err(|e| e.to_string());
    }
    // auto-launch 0.5.0 guards its disable path with `file.exists()` on macOS and Linux
    // but NOT on Windows, where it calls RegDeleteValueW unconditionally and errors with
    // ERROR_FILE_NOT_FOUND when the Run value isn't there. SettingsPage's handleTrayToggle
    // clears launch-at-login on every tray toggle, so without this guard every Windows user
    // who has never enabled autostart would see their tray toggle fail. Checking first makes
    // this setter idempotent everywhere. If is_enabled() itself fails we skip rather than
    // surface it — a disable we can't confirm is needed is not worth failing the caller over.
    // (Edge case: a Run value the user switched off via Task Manager makes is_enabled() report
    // false while the value still exists, so the guard skips deleting it — leaving a
    // stale-but-inert registry value, which is harmless since it can't launch the app.)
    if manager.is_enabled().unwrap_or(false) {
        manager.disable().map_err(|e| e.to_string())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub fn get_launch_at_login_warning() -> &'static str {
    #[cfg(target_os = "linux")]
    return "Starting at login relies on your desktop environment honoring XDG autostart. \
            GNOME, KDE, and XFCE do; a bare window manager may not. The saved entry also \
            records the application's current location, so turn this off and on again after \
            moving or reinstalling the app.";
    #[cfg(not(target_os = "linux"))]
    return "";
}

#[tauri::command]
pub fn get_remote_auto_refresh(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("remote_auto_refresh", "false")? == "true")
}

#[tauri::command]
pub fn set_remote_auto_refresh(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("remote_auto_refresh", if value { "true" } else { "false" })
}

#[tauri::command]
pub fn get_auto_indexing(db: State<'_, AppDb>) -> Result<bool, String> {
    Ok(db.get_setting("auto_indexing", "false")? == "true")
}

#[tauri::command]
pub fn set_auto_indexing(db: State<'_, AppDb>, value: bool) -> Result<(), String> {
    db.set_setting("auto_indexing", if value { "true" } else { "false" })
}

#[tauri::command]
pub async fn cancel_prune(app: tauri::AppHandle, prune_handle: State<'_, PruneHandle>) -> Result<(), String> {
    emit_cancelling(&app, &prune_handle.current_task);
    prune_handle.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut guard) = prune_handle.child.lock() {
        if let Some(ref mut child) = *guard {
            let _ = child.kill();
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct FullDiskAccessStatus {
    pub supported: bool,
    pub granted: bool,
}

#[tauri::command]
pub fn check_full_disk_access() -> Result<FullDiskAccessStatus, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        if home.is_empty() {
            return Ok(FullDiskAccessStatus { supported: true, granted: false });
        }
        let db_path = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
        match std::fs::File::open(&db_path) {
            Ok(_) => Ok(FullDiskAccessStatus { supported: true, granted: true }),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Ok(FullDiskAccessStatus { supported: true, granted: false })
            }
            Err(_) => Ok(FullDiskAccessStatus { supported: true, granted: false }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    Ok(FullDiskAccessStatus { supported: false, granted: false })
}

#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_backend_env, apply_from_repo_flags, apply_from_repo_password, apply_repo_flags,
        apply_repo_password, ensure_writable, last_nonblank_line, merge_credentials,
        parse_raw_data_size, parse_stats_json, validate_init_password, validate_restic_path,
        READ_ONLY_REPO_ERROR,
    };
    use super::super::cache::{Credential, FullRepository};

    // ── validate_init_password ─────────────────────────────────────────────

    #[test]
    fn validate_init_password_rejects_empty() {
        assert_eq!(
            validate_init_password("").unwrap_err(),
            "A password is required to create a repository."
        );
    }

    #[test]
    fn validate_init_password_accepts_non_empty() {
        assert!(validate_init_password("hunter2").is_ok());
    }

    // ── apply_repo_password / apply_from_repo_password ─────────────────────

    #[test]
    fn apply_repo_password_sets_env_for_non_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_repo_password(&mut cmd, "hunter2");
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
        assert!(!cmd.get_args().any(|a| a == "--insecure-no-password"));
    }

    #[test]
    fn apply_repo_password_sets_flag_for_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_repo_password(&mut cmd, "");
        assert!(cmd.get_args().any(|a| a == "--insecure-no-password"));
        assert!(!cmd.get_envs().any(|(k, _)| k == "RESTIC_PASSWORD"));
    }

    #[test]
    fn apply_from_repo_password_sets_env_for_non_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_from_repo_password(&mut cmd, "hunter2");
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_FROM_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
        assert!(!cmd.get_args().any(|a| a == "--from-insecure-no-password"));
    }

    #[test]
    fn apply_from_repo_password_sets_flag_for_empty_password() {
        let mut cmd = std::process::Command::new("restic");
        apply_from_repo_password(&mut cmd, "");
        assert!(cmd.get_args().any(|a| a == "--from-insecure-no-password"));
        assert!(!cmd.get_envs().any(|(k, _)| k == "RESTIC_FROM_PASSWORD"));
    }

    // ── apply_repo_flags / apply_from_repo_flags / ensure_writable ─────────

    #[test]
    fn apply_repo_flags_omits_no_lock_for_writable_repo() {
        let mut cmd = std::process::Command::new("restic");
        let repo = FullRepository { path: "/tmp/repo".into(), password: "hunter2".into(), read_only: false, credentials: vec![] };
        apply_repo_flags(&mut cmd, &repo);
        assert!(!cmd.get_args().any(|a| a == "--no-lock"));
    }

    #[test]
    fn apply_repo_flags_adds_no_lock_for_read_only_repo() {
        let mut cmd = std::process::Command::new("restic");
        let repo = FullRepository { path: "/tmp/repo".into(), password: "hunter2".into(), read_only: true, credentials: vec![] };
        apply_repo_flags(&mut cmd, &repo);
        assert!(cmd.get_args().any(|a| a == "--no-lock"));
        // Password handling is unaffected by read_only.
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
    }

    #[test]
    fn apply_from_repo_flags_adds_no_lock_for_read_only_source() {
        let mut cmd = std::process::Command::new("restic");
        let repo = FullRepository { path: "/tmp/repo".into(), password: "".into(), read_only: true, credentials: vec![] };
        apply_from_repo_flags(&mut cmd, &repo);
        assert!(cmd.get_args().any(|a| a == "--no-lock"));
        assert!(cmd.get_args().any(|a| a == "--from-insecure-no-password"));
    }

    #[test]
    fn ensure_writable_ok_for_writable_repo() {
        let repo = FullRepository { path: "/tmp/repo".into(), password: "".into(), read_only: false, credentials: vec![] };
        assert!(ensure_writable(&repo).is_ok());
    }

    #[test]
    fn ensure_writable_rejects_read_only_repo() {
        let repo = FullRepository { path: "/tmp/repo".into(), password: "".into(), read_only: true, credentials: vec![] };
        assert_eq!(ensure_writable(&repo).unwrap_err(), READ_ONLY_REPO_ERROR);
    }

    // ── apply_backend_env ───────────────────────────────────────────────────

    fn cred(key: &str, value: &str) -> Credential {
        Credential { key: key.to_string(), value: value.to_string() }
    }

    #[test]
    fn apply_backend_env_sets_each_credential() {
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(&mut cmd, &[cred("B2_ACCOUNT_ID", "id"), cred("B2_ACCOUNT_KEY", "key")]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "B2_ACCOUNT_ID" && *v == Some(std::ffi::OsStr::new("id"))));
        assert!(envs.iter().any(|(k, v)| *k == "B2_ACCOUNT_KEY" && *v == Some(std::ffi::OsStr::new("key"))));
    }

    #[test]
    fn apply_backend_env_sets_nothing_for_empty_slice() {
        // The ambient-mode invariant: a repo with no stored credentials must not add
        // any env var at all, so restic's own credential chain is untouched.
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(&mut cmd, &[]);
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn apply_backend_env_skips_empty_valued_entries() {
        // Setting AWS_ACCESS_KEY_ID="" would break minio-go's fallback chain rather
        // than falling through to it — an empty-valued credential must be skipped,
        // not set as an empty string.
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(&mut cmd, &[cred("AWS_ACCESS_KEY_ID", "")]);
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn apply_backend_env_skips_reserved_keys() {
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(
            &mut cmd,
            &[
                cred("PATH", "/evil"),
                cred("RESTIC_PASSWORD", "hunter2"),
                cred("RESTIC_REPOSITORY", "/attacker/repo"),
            ],
        );
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn apply_backend_env_still_sets_non_reserved_keys_alongside_a_reserved_one() {
        // One bad entry in the list must not discard the rest.
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(&mut cmd, &[cred("B2_ACCOUNT_ID", "id"), cred("PATH", "/evil")]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "B2_ACCOUNT_ID" && *v == Some(std::ffi::OsStr::new("id"))));
        assert!(!envs.iter().any(|(k, _)| *k == "PATH"));
    }

    #[test]
    fn credential_cannot_override_restic_repository() {
        // Regression test: every real call site (run_restic_with_path, execute_backup,
        // restore_snapshot, copy_snapshot, mirror_repo, run_one_prune_attempt) sets
        // RESTIC_REPOSITORY on `cmd` *before* calling apply_repo_flags/apply_backend_env.
        // A stored credential named RESTIC_REPOSITORY must not be able to win that
        // collision and redirect the operation to a different repository.
        let mut cmd = std::process::Command::new("restic");
        cmd.env("RESTIC_REPOSITORY", "/real/repo");
        let repo = FullRepository {
            path: "/real/repo".into(),
            password: "hunter2".into(),
            read_only: false,
            credentials: vec![cred("RESTIC_REPOSITORY", "/attacker/repo")],
        };
        apply_repo_flags(&mut cmd, &repo);
        let envs: Vec<_> = cmd.get_envs().collect();
        let repo_env = envs.iter().find(|(k, _)| *k == "RESTIC_REPOSITORY").unwrap();
        assert_eq!(repo_env.1, Some(std::ffi::OsStr::new("/real/repo")));
    }

    #[test]
    fn apply_backend_env_sets_allowlisted_rest_credentials() {
        // The allowlist in backends::is_reserved_key must hold at apply time too —
        // apply_backend_env filters independently of validate_credentials.
        let mut cmd = std::process::Command::new("restic");
        apply_backend_env(
            &mut cmd,
            &[cred("RESTIC_REST_USERNAME", "u"), cred("RESTIC_REST_PASSWORD", "pass/word")],
        );
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_REST_USERNAME"
            && *v == Some(std::ffi::OsStr::new("u"))));
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_REST_PASSWORD"
            && *v == Some(std::ffi::OsStr::new("pass/word"))));
    }

    #[test]
    fn apply_backend_env_sets_rest_credentials_but_still_skips_reserved_ones() {
        // The allowlist must not become a hole: a RESTIC_REPOSITORY row alongside a
        // legitimate REST pair is still dropped.
        let mut cmd = std::process::Command::new("restic");
        cmd.env("RESTIC_REPOSITORY", "rest:https://real/");
        let repo = FullRepository {
            path: "rest:https://real/".into(),
            password: "hunter2".into(),
            read_only: false,
            credentials: vec![
                cred("RESTIC_REST_PASSWORD", "pass/word"),
                cred("RESTIC_REPOSITORY", "rest:https://attacker/"),
            ],
        };
        apply_repo_flags(&mut cmd, &repo);
        let envs: Vec<_> = cmd.get_envs().collect();
        let repo_env = envs.iter().find(|(k, _)| *k == "RESTIC_REPOSITORY").unwrap();
        assert_eq!(repo_env.1, Some(std::ffi::OsStr::new("rest:https://real/")));
        assert!(envs.iter().any(|(k, _)| *k == "RESTIC_REST_PASSWORD"));
    }

    #[test]
    fn apply_repo_flags_applies_credentials_before_password() {
        let mut cmd = std::process::Command::new("restic");
        let repo = FullRepository {
            path: "b2:bucket:path".into(),
            password: "hunter2".into(),
            read_only: false,
            credentials: vec![cred("B2_ACCOUNT_ID", "id")],
        };
        apply_repo_flags(&mut cmd, &repo);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "B2_ACCOUNT_ID" && *v == Some(std::ffi::OsStr::new("id"))));
        assert!(envs.iter().any(|(k, v)| *k == "RESTIC_PASSWORD" && *v == Some(std::ffi::OsStr::new("hunter2"))));
    }

    // ── merge_credentials ───────────────────────────────────────────────────

    #[test]
    fn merge_credentials_merges_disjoint_sets() {
        let dest = vec![cred("AWS_ACCESS_KEY_ID", "a")];
        let src = vec![cred("B2_ACCOUNT_ID", "b")];
        let mut merged = merge_credentials(&dest, &src).unwrap();
        merged.sort_by(|a, b| a.key.cmp(&b.key));
        let keys: Vec<&str> = merged.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, vec!["AWS_ACCESS_KEY_ID", "B2_ACCOUNT_ID"]);
    }

    #[test]
    fn merge_credentials_allows_identical_duplicate_values() {
        let dest = vec![cred("B2_ACCOUNT_ID", "same")];
        let src = vec![cred("B2_ACCOUNT_ID", "same")];
        let merged = merge_credentials(&dest, &src).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].value, "same");
    }

    #[test]
    fn merge_credentials_errors_and_names_the_key_on_conflict() {
        let dest = vec![cred("B2_ACCOUNT_ID", "account-a")];
        let src = vec![cred("B2_ACCOUNT_ID", "account-b")];
        match merge_credentials(&dest, &src) {
            Err(err) => assert!(err.contains("B2_ACCOUNT_ID")),
            Ok(_) => panic!("expected merge_credentials to reject a conflicting value"),
        }
    }

    #[test]
    fn merge_credentials_handles_empty_inputs() {
        assert!(merge_credentials(&[], &[]).unwrap().is_empty());
        let dest = vec![cred("K", "v")];
        assert_eq!(merge_credentials(&dest, &[]).unwrap().len(), 1);
        assert_eq!(merge_credentials(&[], &dest).unwrap().len(), 1);
    }

    #[test]
    fn merge_credentials_rejects_two_rest_repos_with_different_passwords() {
        // restic reads one unprefixed RESTIC_REST_PASSWORD for both sides of a copy —
        // there is no RESTIC_FROM_ counterpart — so this pairing is genuinely impossible
        // and must fail pre-flight rather than as a confusing restic-level 401.
        let dest = vec![cred("RESTIC_REST_PASSWORD", "dest-pw")];
        let src = vec![cred("RESTIC_REST_PASSWORD", "src-pw")];
        assert!(merge_credentials(&dest, &src).is_err());
    }

    #[test]
    fn merge_credentials_allows_rest_source_with_non_rest_destination() {
        let dest = vec![cred("B2_ACCOUNT_ID", "id")];
        let src = vec![cred("RESTIC_REST_PASSWORD", "pw")];
        let merged = merge_credentials(&dest, &src).unwrap();
        assert_eq!(merged.len(), 2);
    }

    // ── last_nonblank_line / parse_stats_json ──────────────────────────────

    #[test]
    fn last_nonblank_line_finds_single_line() {
        assert_eq!(last_nonblank_line(r#"{"total_size":1}"#), Some(r#"{"total_size":1}"#));
    }

    #[test]
    fn last_nonblank_line_skips_trailing_blank_lines() {
        let stdout = "{\"total_size\":1}\n\n   \n";
        assert_eq!(last_nonblank_line(stdout), Some(r#"{"total_size":1}"#));
    }

    #[test]
    fn last_nonblank_line_picks_last_of_multiple_json_lines() {
        // restic can emit progress/status lines before the final summary line.
        let stdout = "{\"message_type\":\"status\"}\n{\"total_size\":42,\"total_file_count\":3,\"snapshots_count\":1}\n";
        assert_eq!(
            last_nonblank_line(stdout),
            Some(r#"{"total_size":42,"total_file_count":3,"snapshots_count":1}"#)
        );
    }

    #[test]
    fn last_nonblank_line_all_blank_returns_none() {
        assert_eq!(last_nonblank_line("\n  \n\t\n"), None);
        assert_eq!(last_nonblank_line(""), None);
    }

    #[test]
    fn parse_stats_json_well_formed() {
        let stdout = r#"{"total_size":100,"total_file_count":10,"snapshots_count":2}"#;
        let stats = parse_stats_json(stdout).unwrap();
        assert_eq!(stats.total_size, 100);
        assert_eq!(stats.total_file_count, 10);
        assert_eq!(stats.snapshots_count, 2);
    }

    #[test]
    fn parse_stats_json_missing_fields_default_to_zero() {
        let stats = parse_stats_json(r#"{"total_size":5}"#).unwrap();
        assert_eq!(stats.total_size, 5);
        assert_eq!(stats.total_file_count, 0);
        assert_eq!(stats.snapshots_count, 0);
    }

    #[test]
    fn parse_stats_json_empty_stdout_is_error() {
        let err = parse_stats_json("").unwrap_err();
        assert!(err.contains("No output"), "unexpected error: {err}");
    }

    #[test]
    fn parse_stats_json_malformed_json_is_error() {
        assert!(parse_stats_json("not json").is_err());
    }

    // ── parse_raw_data_size ─────────────────────────────────────────────────

    #[test]
    fn parse_raw_data_size_well_formed() {
        let stdout = r#"{"total_size":100,"total_uncompressed_size":400}"#;
        assert_eq!(parse_raw_data_size(stdout).unwrap(), 100);
    }

    #[test]
    fn parse_raw_data_size_missing_field_defaults_to_zero() {
        assert_eq!(parse_raw_data_size(r#"{"total_uncompressed_size":400}"#).unwrap(), 0);
    }

    #[test]
    fn parse_raw_data_size_empty_stdout_is_error() {
        let err = parse_raw_data_size("").unwrap_err();
        assert!(err.contains("No output"), "unexpected error: {err}");
    }

    #[test]
    fn parse_raw_data_size_malformed_json_is_error() {
        assert!(parse_raw_data_size("not json").is_err());
    }

    // ── validate_restic_path ────────────────────────────────────────────────

    #[test]
    fn validate_restic_path_rejects_empty() {
        assert!(validate_restic_path("").is_err());
    }

    #[test]
    fn validate_restic_path_accepts_bare_command_name() {
        // "restic" alone (no path separator) is never checked against the filesystem —
        // it's resolved against $PATH at call time, not by this validator.
        assert!(validate_restic_path("restic").is_ok());
    }

    #[test]
    fn validate_restic_path_accepts_existing_absolute_file() {
        // The current test binary is guaranteed to exist at an absolute path.
        let exe = std::env::current_exe().unwrap();
        assert!(validate_restic_path(exe.to_str().unwrap()).is_ok());
    }

    #[test]
    fn validate_restic_path_rejects_nonexistent_absolute_file() {
        let err = validate_restic_path("/nonexistent/xyz/restic").unwrap_err();
        assert!(err.contains("No file found"), "unexpected error: {err}");
    }

    #[test]
    fn validate_restic_path_rejects_nonexistent_windows_style_paths() {
        assert!(validate_restic_path(r"C:\nonexistent\restic.exe").is_err());
        assert!(validate_restic_path(r"\nonexistent\restic").is_err());
    }
}
