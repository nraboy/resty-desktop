use std::collections::{HashMap, HashSet};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::State;
use zeroize::Zeroize;

use super::cache::{AppDb, BackupPlan, ImportRepo, MasterKey, RetentionPolicy, Schedule};
use super::crypto;
use super::schedule::next_fire_time;

const BUNDLE_VERSION: u32 = 1;
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

// ── bundle file format ──────────────────────────────────────────────────────
//
// The exported `.json` file is readable; only repository passwords are encrypted
// (with a key derived from a user-supplied export passphrase). Every object
// carries its own `id` and references other objects by that id, so the file is
// self-describing and safe to inspect or hand-edit. On import every object gets
// a fresh id and references are remapped (import-as-copies).

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportBundle {
    app: String,
    /// Schema version of the bundle format (what the importer validates).
    version: u32,
    /// Resty app version that produced the file — informational, for debugging.
    #[serde(default)]
    app_version: String,
    exported_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    encryption: Option<ExportEncryption>,
    repositories: Vec<ExportRepo>,
    backup_plans: Vec<ExportPlan>,
    schedules: Vec<ExportSchedule>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportEncryption {
    kdf: String,
    salt: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportRepo {
    id: String,
    name: String,
    path: String,
    password: EncSecret,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncSecret {
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportPlan {
    id: String,
    name: String,
    repo_id: String,
    paths: Vec<String>,
    tags: Vec<String>,
    excludes: Vec<String>,
    retention: Option<RetentionPolicy>,
    limit_upload: Option<u32>,
    limit_download: Option<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportSchedule {
    id: String,
    name: String,
    plan_ids: Vec<String>,
    cron_expr: String,
    enabled: bool,
}

// ── command return types ────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub repos: u32,
    pub plans: u32,
    pub schedules: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub repos: u32,
    pub plans: u32,
    pub schedules: u32,
    pub requires_password: bool,
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Returns a name not already in `used`, appending " (imported)" (then numbered
/// variants) on collision. Records the chosen name in `used`.
fn uniquify(base: &str, used: &mut HashSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    let mut candidate = format!("{base} (imported)");
    let mut n = 2;
    while !used.insert(candidate.clone()) {
        candidate = format!("{base} (imported {n})");
        n += 1;
    }
    candidate
}

fn parse_bundle(file_path: &str) -> Result<ExportBundle, String> {
    let raw = std::fs::read_to_string(file_path).map_err(|e| format!("Could not read file: {e}"))?;
    let bundle: ExportBundle =
        serde_json::from_str(&raw).map_err(|_| "This is not a valid Resty Desktop export file.".to_string())?;
    if bundle.app != "resty" {
        return Err("This is not a valid Resty Desktop export file.".to_string());
    }
    if bundle.version != BUNDLE_VERSION {
        return Err(format!(
            "Unsupported export version {}. This app supports version {}.",
            bundle.version, BUNDLE_VERSION
        ));
    }
    Ok(bundle)
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── export ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn export_data(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    out_path: String,
    export_password: Option<String>,
) -> Result<ExportSummary, String> {
    let key = master_key.get()?;

    // The bundle is a full snapshot of the user's config — always everything.
    let all_repos = db.list_repos()?;
    let all_plans = db.list_backup_plans()?;
    let all_schedules = db.list_schedules()?;

    // Encryption is required whenever the bundle carries repo passwords.
    let encryption = if all_repos.is_empty() {
        None
    } else {
        let pw = export_password
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or_else(|| "An export passphrase is required when exporting repositories.".to_string())?;
        let salt = crypto::random_bytes::<16>();
        let export_key = crypto::derive_key(pw, &salt)?;
        Some((export_key, salt))
    };

    let mut repositories = Vec::with_capacity(all_repos.len());
    if let Some((export_key, _)) = &encryption {
        for r in &all_repos {
            let full = db.get_full_repo(&r.id, &key)?;
            // A passwordless repo (empty stored password) carries no secret, so
            // write empty nonce/ciphertext rather than encrypting empty bytes
            // (which would still emit a nonce + auth tag and look like a real
            // secret). decrypt_secret short-circuits the empty form on import.
            let password = if full.password.is_empty() {
                EncSecret { nonce: String::new(), ciphertext: String::new() }
            } else {
                let (nonce, ct) = crypto::encrypt(export_key, full.password.as_bytes())?;
                EncSecret { nonce: B64.encode(nonce), ciphertext: B64.encode(ct) }
            };
            repositories.push(ExportRepo {
                id: r.id.clone(),
                name: r.name.clone(),
                path: r.path.clone(),
                password,
            });
        }
    }

    // Export every plan and schedule verbatim, including plans whose repository
    // was deleted (orphaned) — they keep their config and are imported with no
    // repository assigned (see import_data), matching how the editor treats
    // orphans. Dangling references are tolerated on import, not dropped here.
    let backup_plans: Vec<ExportPlan> = all_plans
        .iter()
        .map(|p| ExportPlan {
            id: p.id.clone(),
            name: p.name.clone(),
            repo_id: p.repo_id.clone(),
            paths: p.paths.clone(),
            tags: p.tags.clone(),
            excludes: p.excludes.clone(),
            retention: p.retention.clone(),
            limit_upload: p.limit_upload,
            limit_download: p.limit_download,
        })
        .collect();

    let schedules: Vec<ExportSchedule> = all_schedules
        .iter()
        .map(|s| ExportSchedule {
            id: s.id.clone(),
            name: s.name.clone(),
            plan_ids: s.plan_ids.clone(),
            cron_expr: s.cron_expr.clone(),
            enabled: s.enabled,
        })
        .collect();

    let summary = ExportSummary {
        repos: repositories.len() as u32,
        plans: backup_plans.len() as u32,
        schedules: schedules.len() as u32,
    };

    let bundle = ExportBundle {
        app: "resty".to_string(),
        version: BUNDLE_VERSION,
        app_version: app.config().version.clone().unwrap_or_default(),
        exported_at: now_ts(),
        encryption: encryption.as_ref().map(|(_, salt)| ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode(salt),
        }),
        repositories,
        backup_plans,
        schedules,
    };

    let json = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    std::fs::write(&out_path, json).map_err(|e| format!("Could not write file: {e}"))?;
    Ok(summary)
}

// ── import ──────────────────────────────────────────────────────────────────

/// Derive the export key from the bundle's stored salt + the user's passphrase.
fn derive_export_key(enc: &ExportEncryption, password: Option<&str>) -> Result<[u8; 32], String> {
    let pw = password
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "This export is password-protected. Enter the export passphrase.".to_string())?;
    let salt = B64.decode(&enc.salt).map_err(|e| e.to_string())?;
    crypto::derive_key(pw, &salt)
}

fn decrypt_secret(key: &[u8; 32], secret: &EncSecret) -> Result<Vec<u8>, String> {
    // An empty secret denotes a passwordless repo (no secret to protect). Treat
    // it as an empty password without attempting a decrypt. Old bundles that
    // encrypted empty bytes still carry a real nonce+tag and take the decrypt
    // path below (decrypting to empty) — so this stays backward compatible.
    if secret.nonce.is_empty() && secret.ciphertext.is_empty() {
        return Ok(Vec::new());
    }
    let nonce = B64.decode(&secret.nonce).map_err(|e| e.to_string())?;
    let ct = B64.decode(&secret.ciphertext).map_err(|e| e.to_string())?;
    crypto::decrypt(key, &nonce, &ct)
        .map_err(|_| "Incorrect export passphrase.".to_string())
}

#[tauri::command]
pub fn preview_import(file_path: String, export_password: Option<String>) -> Result<ImportPreview, String> {
    let bundle = parse_bundle(&file_path)?;
    let requires_password = bundle.encryption.is_some();

    // Verify the passphrase early if one was supplied (counts/paths are readable
    // regardless, so previewing without a passphrase is allowed).
    if let (Some(enc), Some(pw)) = (&bundle.encryption, export_password.as_deref()) {
        if !pw.is_empty() {
            let key = derive_export_key(enc, Some(pw))?;
            // Verify the passphrase against a repo that actually carries a
            // secret. A passwordless repo's empty secret would decrypt to
            // nothing without exercising the key, letting a wrong passphrase
            // pass preview. If every repo is passwordless there's no secret to
            // test against — skip (import still enforces a non-empty passphrase
            // via derive_export_key).
            if let Some(with_secret) = bundle
                .repositories
                .iter()
                .find(|r| !r.password.nonce.is_empty() || !r.password.ciphertext.is_empty())
            {
                let mut plaintext = decrypt_secret(&key, &with_secret.password)?;
                plaintext.zeroize();
            }
        }
    }

    Ok(ImportPreview {
        repos: bundle.repositories.len() as u32,
        plans: bundle.backup_plans.len() as u32,
        schedules: bundle.schedules.len() as u32,
        requires_password,
    })
}

#[tauri::command]
pub fn import_data(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    file_path: String,
    export_password: Option<String>,
) -> Result<ExportSummary, String> {
    let key = master_key.get()?;
    let bundle = parse_bundle(&file_path)?;

    let export_key = match &bundle.encryption {
        Some(enc) => Some(derive_export_key(enc, export_password.as_deref())?),
        None => None,
    };

    // Name sets seeded from existing data so imported copies never silently
    // collide; each type has its own namespace.
    let mut repo_names: HashSet<String> = db.list_repos()?.into_iter().map(|r| r.name).collect();
    let mut plan_names: HashSet<String> = db.list_backup_plans()?.into_iter().map(|p| p.name).collect();
    let mut sched_names: HashSet<String> = db.list_schedules()?.into_iter().map(|s| s.name).collect();

    // Repositories: decrypt with the export key, re-encrypt with the local
    // master key. Fresh UUIDs build the old-id → new-id map for remapping.
    let mut repos: Vec<ImportRepo> = Vec::with_capacity(bundle.repositories.len());
    let mut repo_id_map: HashMap<String, String> = HashMap::new();
    for r in &bundle.repositories {
        let ekey = export_key
            .as_ref()
            .ok_or_else(|| "This export contains repositories but is not encrypted.".to_string())?;
        let mut password = decrypt_secret(ekey, &r.password)?;
        let (nonce, ciphertext) = crypto::encrypt(&key, &password)?;
        password.zeroize();
        let new_id = uuid::Uuid::new_v4().to_string();
        repo_id_map.insert(r.id.clone(), new_id.clone());
        repos.push(ImportRepo {
            id: new_id,
            name: uniquify(&r.name, &mut repo_names),
            path: r.path.clone(),
            password_nonce: nonce,
            password_ciphertext: ciphertext,
        });
    }

    // Backup plans: remap repoId → new repo id.
    let mut plans: Vec<BackupPlan> = Vec::with_capacity(bundle.backup_plans.len());
    let mut plan_id_map: HashMap<String, String> = HashMap::new();
    for p in &bundle.backup_plans {
        // If the plan's repository isn't in the file (it was orphaned by a repo
        // deletion before export), import it with no repository assigned — the
        // user can reassign one in the editor, matching how orphans behave today.
        let repo_id = repo_id_map.get(&p.repo_id).cloned().unwrap_or_default();
        let new_id = uuid::Uuid::new_v4().to_string();
        plan_id_map.insert(p.id.clone(), new_id.clone());
        plans.push(BackupPlan {
            id: new_id,
            name: uniquify(&p.name, &mut plan_names),
            repo_id,
            paths: p.paths.clone(),
            tags: p.tags.clone(),
            excludes: p.excludes.clone(),
            retention: p.retention.clone(),
            limit_upload: p.limit_upload,
            limit_download: p.limit_download,
        });
    }

    // Schedules: remap planIds → new plan ids; recompute timing for this host.
    let now = now_ts();
    let mut schedules: Vec<Schedule> = Vec::with_capacity(bundle.schedules.len());
    for s in &bundle.schedules {
        // Keep only references to plans present in this file; drop any dangling
        // plan reference rather than failing the whole import.
        let plan_ids: Vec<String> = s
            .plan_ids
            .iter()
            .filter_map(|old_pid| plan_id_map.get(old_pid).cloned())
            .collect();
        schedules.push(Schedule {
            id: uuid::Uuid::new_v4().to_string(),
            name: uniquify(&s.name, &mut sched_names),
            plan_ids,
            // A bad cron shouldn't abort the whole import; leave it unscheduled.
            next_run_at: next_fire_time(&s.cron_expr).ok(),
            cron_expr: s.cron_expr.clone(),
            // Always import disabled: paths may need adjusting on the new host, so
            // don't let backups start firing automatically until the user reviews.
            enabled: false,
            last_run_at: None,
            created_at: now,
        });
    }

    db.import_bundle(&repos, &plans, &schedules)?;

    Ok(ExportSummary {
        repos: repos.len() as u32,
        plans: plans.len() as u32,
        schedules: schedules.len() as u32,
    })
}

// ── Backrest import ───────────────────────────────────────────────────────────
//
// Backrest (github.com/garethgeorge/backrest) stores its whole state in a single
// protobuf-derived `config.json` (snake_case fields rendered as camelCase). We
// parse only the fields that map onto Resty's model and silently drop the rest
// (hooks, restic flags/env, prune/check policies, clock settings, hourly
// retention, frequency-based schedules). Repo passwords are plaintext in
// Backrest, so we re-encrypt them under the local master key — no export
// passphrase is involved. Every field is optional so partial/older configs parse.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestConfig {
    #[serde(default)]
    repos: Vec<BackrestRepo>,
    #[serde(default)]
    plans: Vec<BackrestPlan>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestRepo {
    #[serde(default)]
    id: String,
    #[serde(default)]
    uri: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    env: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestPlan {
    #[serde(default)]
    id: String,
    #[serde(default)]
    repo: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    excludes: Vec<String>,
    #[serde(default)]
    iexcludes: Vec<String>,
    #[serde(default)]
    schedule: Option<BackrestSchedule>,
    #[serde(default)]
    retention: Option<BackrestRetention>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestSchedule {
    #[serde(default)]
    cron: Option<String>,
    #[serde(default)]
    disabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestRetention {
    #[serde(default)]
    policy_keep_last_n: Option<u32>,
    #[serde(default)]
    policy_time_bucketed: Option<BackrestTimeBucketed>,
    // `policyKeepAll` is ignored: keeping everything maps to no retention, which
    // is also what an absent policy yields, so it falls through to `None`.
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackrestTimeBucketed {
    // `hourly` has no Resty equivalent and is dropped.
    #[serde(default)]
    daily: Option<u32>,
    #[serde(default)]
    weekly: Option<u32>,
    #[serde(default)]
    monthly: Option<u32>,
    #[serde(default)]
    yearly: Option<u32>,
    #[serde(default)]
    keep_last_n: Option<u32>,
}

fn parse_backrest(file_path: &str) -> Result<BackrestConfig, String> {
    let raw = std::fs::read_to_string(file_path).map_err(|e| format!("Could not read file: {e}"))?;
    parse_backrest_str(&raw)
}

fn parse_backrest_str(raw: &str) -> Result<BackrestConfig, String> {
    let cfg: BackrestConfig = serde_json::from_str(raw)
        .map_err(|_| "This is not a valid Backrest config file.".to_string())?;
    if cfg.repos.is_empty() && cfg.plans.is_empty() {
        return Err("No repositories or backup plans found in this Backrest config.".to_string());
    }
    Ok(cfg)
}

impl BackrestRetention {
    /// Map Backrest's retention oneof onto Resty's. `policyKeepAll` and an
    /// all-empty policy yield `None` (no retention); `hourly` is not representable.
    fn to_resty(&self) -> Option<RetentionPolicy> {
        if let Some(n) = self.policy_keep_last_n {
            return Some(RetentionPolicy {
                keep_last: Some(n),
                keep_daily: None,
                keep_weekly: None,
                keep_monthly: None,
                keep_yearly: None,
            });
        }
        if let Some(tb) = &self.policy_time_bucketed {
            let policy = RetentionPolicy {
                keep_last: tb.keep_last_n,
                keep_daily: tb.daily,
                keep_weekly: tb.weekly,
                keep_monthly: tb.monthly,
                keep_yearly: tb.yearly,
            };
            let empty = policy.keep_last.is_none()
                && policy.keep_daily.is_none()
                && policy.keep_weekly.is_none()
                && policy.keep_monthly.is_none()
                && policy.keep_yearly.is_none();
            return if empty { None } else { Some(policy) };
        }
        None
    }
}

/// Backrest repo passwords live in the `password` field, or as a
/// `RESTIC_PASSWORD=...` entry in `env`. Returns "" when neither is present
/// (the repo still imports; the user supplies a password by editing it).
fn backrest_repo_password(r: &BackrestRepo) -> String {
    if !r.password.is_empty() {
        return r.password.clone();
    }
    r.env
        .iter()
        .find_map(|e| e.strip_prefix("RESTIC_PASSWORD=").map(str::to_string))
        .unwrap_or_default()
}

/// Count the schedules we'd synthesize: one per plan with an enabled cron.
fn backrest_schedule_count(cfg: &BackrestConfig) -> u32 {
    cfg.plans
        .iter()
        .filter(|p| {
            p.schedule
                .as_ref()
                .is_some_and(|s| s.disabled != Some(true) && s.cron.as_deref().is_some_and(|c| !c.is_empty()))
        })
        .count() as u32
}

#[tauri::command]
pub fn preview_backrest_import(file_path: String) -> Result<ImportPreview, String> {
    let cfg = parse_backrest(&file_path)?;
    Ok(ImportPreview {
        repos: cfg.repos.len() as u32,
        plans: cfg.plans.len() as u32,
        schedules: backrest_schedule_count(&cfg),
        requires_password: false,
    })
}

#[tauri::command]
pub fn import_backrest_config(
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    file_path: String,
) -> Result<ExportSummary, String> {
    let key = master_key.get()?;
    let cfg = parse_backrest(&file_path)?;

    let mut repo_names: HashSet<String> = db.list_repos()?.into_iter().map(|r| r.name).collect();
    let mut plan_names: HashSet<String> = db.list_backup_plans()?.into_iter().map(|p| p.name).collect();
    let mut sched_names: HashSet<String> = db.list_schedules()?.into_iter().map(|s| s.name).collect();

    // Repositories: re-encrypt the plaintext password under the master key.
    let mut repos: Vec<ImportRepo> = Vec::with_capacity(cfg.repos.len());
    let mut repo_id_map: HashMap<String, String> = HashMap::new();
    for r in &cfg.repos {
        let mut password = backrest_repo_password(r).into_bytes();
        let (nonce, ciphertext) = crypto::encrypt(&key, &password)?;
        password.zeroize();
        let new_id = uuid::Uuid::new_v4().to_string();
        repo_id_map.insert(r.id.clone(), new_id.clone());
        // Backrest's repo `id` is a human-readable label, so it doubles as a name.
        let name = if r.id.is_empty() { "Imported repository".to_string() } else { r.id.clone() };
        repos.push(ImportRepo {
            id: new_id,
            name: uniquify(&name, &mut repo_names),
            path: r.uri.clone(),
            password_nonce: nonce,
            password_ciphertext: ciphertext,
        });
    }

    // Plans: remap repo ref, fold case-insensitive excludes into excludes,
    // map retention. Tags and bandwidth limits have no Backrest source.
    let mut plans: Vec<BackupPlan> = Vec::with_capacity(cfg.plans.len());
    let mut plan_id_map: HashMap<String, String> = HashMap::new();
    for p in &cfg.plans {
        let repo_id = repo_id_map.get(&p.repo).cloned().unwrap_or_default();
        let new_id = uuid::Uuid::new_v4().to_string();
        plan_id_map.insert(p.id.clone(), new_id.clone());
        let mut excludes = p.excludes.clone();
        excludes.extend(p.iexcludes.iter().cloned());
        let name = if p.id.is_empty() { "Imported plan".to_string() } else { p.id.clone() };
        plans.push(BackupPlan {
            id: new_id,
            name: uniquify(&name, &mut plan_names),
            repo_id,
            paths: p.paths.clone(),
            tags: Vec::new(),
            excludes,
            retention: p.retention.as_ref().and_then(BackrestRetention::to_resty),
            limit_upload: None,
            limit_download: None,
        });
    }

    // Schedules: Backrest embeds one schedule per plan. Synthesize a Resty
    // Schedule for every plan with an enabled cron; frequency-based and disabled
    // schedules are dropped.
    let now = now_ts();
    let mut schedules: Vec<Schedule> = Vec::new();
    for p in &cfg.plans {
        let Some(sched) = &p.schedule else { continue };
        if sched.disabled == Some(true) {
            continue;
        }
        let Some(cron) = sched.cron.as_deref().filter(|c| !c.is_empty()) else { continue };
        let Some(plan_new_id) = plan_id_map.get(&p.id) else { continue };
        let base = if p.id.is_empty() { "Imported schedule".to_string() } else { format!("{} schedule", p.id) };
        schedules.push(Schedule {
            id: uuid::Uuid::new_v4().to_string(),
            name: uniquify(&base, &mut sched_names),
            plan_ids: vec![plan_new_id.clone()],
            next_run_at: next_fire_time(cron).ok(),
            cron_expr: cron.to_string(),
            // Import disabled so backups don't fire before paths are reviewed.
            enabled: false,
            last_run_at: None,
            created_at: now,
        });
    }

    db.import_bundle(&repos, &plans, &schedules)?;

    Ok(ExportSummary {
        repos: repos.len() as u32,
        plans: plans.len() as u32,
        schedules: schedules.len() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── uniquify ────────────────────────────────────────────────────────────

    #[test]
    fn uniquify_returns_original_when_no_collision() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(uniquify("My Repo", &mut used), "My Repo");
        assert!(used.contains("My Repo"));
    }

    #[test]
    fn uniquify_first_collision_appends_imported() {
        let mut used = std::collections::HashSet::new();
        used.insert("My Repo".to_string());
        assert_eq!(uniquify("My Repo", &mut used), "My Repo (imported)");
        assert!(used.contains("My Repo (imported)"));
    }

    #[test]
    fn uniquify_second_collision_appends_numbered() {
        let mut used = std::collections::HashSet::new();
        used.insert("X".to_string());
        used.insert("X (imported)".to_string());
        assert_eq!(uniquify("X", &mut used), "X (imported 2)");
    }

    #[test]
    fn uniquify_third_collision() {
        let mut used = std::collections::HashSet::new();
        used.insert("X".to_string());
        used.insert("X (imported)".to_string());
        used.insert("X (imported 2)".to_string());
        assert_eq!(uniquify("X", &mut used), "X (imported 3)");
    }

    #[test]
    fn uniquify_records_chosen_name_in_used_set() {
        let mut used = std::collections::HashSet::new();
        let name = uniquify("Repo", &mut used);
        // Calling again with the same base must not return the same name.
        let name2 = uniquify("Repo", &mut used);
        assert_ne!(name, name2);
    }

    // ── decrypt_secret / derive_export_key ─────────────────────────────────

    #[test]
    fn decrypt_secret_round_trip() {
        let key = [1u8; 32];
        let plaintext = b"super-secret-repo-password";
        let (nonce, ct) = super::super::crypto::encrypt(&key, plaintext).unwrap();
        let enc = EncSecret {
            nonce: B64.encode(&nonce),
            ciphertext: B64.encode(&ct),
        };
        let decrypted = decrypt_secret(&key, &enc).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_secret_empty_secret_yields_empty_password() {
        // Passwordless repos export as an empty secret; import must read that
        // back as an empty password without attempting (and failing) a decrypt.
        let key = [1u8; 32];
        let enc = EncSecret { nonce: String::new(), ciphertext: String::new() };
        let decrypted = decrypt_secret(&key, &enc).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn decrypt_secret_wrong_key_returns_passphrase_error() {
        let key = [1u8; 32];
        let wrong_key = [2u8; 32];
        let (nonce, ct) = super::super::crypto::encrypt(&key, b"secret").unwrap();
        let enc = EncSecret {
            nonce: B64.encode(&nonce),
            ciphertext: B64.encode(&ct),
        };
        let err = decrypt_secret(&wrong_key, &enc).unwrap_err();
        assert!(err.contains("Incorrect export passphrase"));
    }

    #[test]
    fn derive_export_key_requires_non_empty_password() {
        let enc = ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode([0u8; 16]),
        };
        assert!(derive_export_key(&enc, None).is_err());
        assert!(derive_export_key(&enc, Some("")).is_err());
    }

    #[test]
    fn derive_export_key_is_deterministic() {
        let enc = ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode([0u8; 16]),
        };
        let k1 = derive_export_key(&enc, Some("mypass")).unwrap();
        let k2 = derive_export_key(&enc, Some("mypass")).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_export_key_differs_on_different_passwords() {
        let enc = ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode([0u8; 16]),
        };
        let k1 = derive_export_key(&enc, Some("pass1")).unwrap();
        let k2 = derive_export_key(&enc, Some("pass2")).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_export_key_differs_on_different_salts() {
        let enc1 = ExportEncryption { kdf: "argon2id".to_string(), salt: B64.encode([0u8; 16]) };
        let enc2 = ExportEncryption { kdf: "argon2id".to_string(), salt: B64.encode([1u8; 16]) };
        let k1 = derive_export_key(&enc1, Some("same-pass")).unwrap();
        let k2 = derive_export_key(&enc2, Some("same-pass")).unwrap();
        assert_ne!(k1, k2);
    }

    // ── export bundle passphrase end-to-end ─────────────────────────────────

    #[test]
    fn export_key_to_encrypt_then_decrypt_round_trip() {
        let enc = ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode([7u8; 16]),
        };
        let export_key = derive_export_key(&enc, Some("my-export-pass")).unwrap();
        let (nonce, ct) = super::super::crypto::encrypt(&export_key, b"repo-password-123").unwrap();
        let secret = EncSecret {
            nonce: B64.encode(&nonce),
            ciphertext: B64.encode(&ct),
        };
        let decrypted = decrypt_secret(&export_key, &secret).unwrap();
        assert_eq!(decrypted, b"repo-password-123");
    }

    #[test]
    fn export_key_wrong_passphrase_fails_decrypt() {
        let enc = ExportEncryption {
            kdf: "argon2id".to_string(),
            salt: B64.encode([7u8; 16]),
        };
        let export_key = derive_export_key(&enc, Some("correct-pass")).unwrap();
        let wrong_key = derive_export_key(&enc, Some("wrong-pass")).unwrap();
        let (nonce, ct) = super::super::crypto::encrypt(&export_key, b"secret").unwrap();
        let secret = EncSecret {
            nonce: B64.encode(&nonce),
            ciphertext: B64.encode(&ct),
        };
        assert!(decrypt_secret(&wrong_key, &secret).is_err());
    }
}

#[cfg(test)]
mod backrest_tests {
    use super::*;

    /// Representative Backrest config with fake data covering every mapping case:
    /// plaintext / env / missing passwords, cron / frequency / disabled schedules,
    /// keep-last / time-bucketed / keep-all retention, merged excludes, a dangling
    /// repo reference, and ignored fields (hooks, prune/check policies, auth).
    const FIXTURE: &str = include_str!("../../tests/fixtures/backrest_config.json");

    fn parsed() -> BackrestConfig {
        parse_backrest_str(FIXTURE).expect("fixture should parse")
    }

    fn plan<'a>(cfg: &'a BackrestConfig, id: &str) -> &'a BackrestPlan {
        cfg.plans.iter().find(|p| p.id == id).expect("plan present")
    }

    fn repo<'a>(cfg: &'a BackrestConfig, id: &str) -> &'a BackrestRepo {
        cfg.repos.iter().find(|r| r.id == id).expect("repo present")
    }

    #[test]
    fn parses_counts_ignoring_unknown_fields() {
        let cfg = parsed();
        assert_eq!(cfg.repos.len(), 4);
        assert_eq!(cfg.plans.len(), 5);
    }

    #[test]
    fn rejects_invalid_and_empty_configs() {
        assert!(parse_backrest_str("not json").is_err());
        assert!(parse_backrest_str(r#"{"repos":[],"plans":[]}"#).is_err());
    }

    #[test]
    fn resolves_passwords_from_field_then_env_then_none() {
        let cfg = parsed();
        assert_eq!(backrest_repo_password(repo(&cfg, "plaintext-repo")), "fake-plaintext-password");
        assert_eq!(backrest_repo_password(repo(&cfg, "env-repo")), "fake-env-password");
        assert_eq!(backrest_repo_password(repo(&cfg, "nopass-repo")), "");
    }

    #[test]
    fn counts_only_enabled_cron_schedules() {
        // keep-last + bucketed + dangling have crons; frequency-based and disabled
        // schedules are not synthesized.
        assert_eq!(backrest_schedule_count(&parsed()), 3);
    }

    #[test]
    fn maps_keep_last_retention() {
        let cfg = parsed();
        let r = plan(&cfg, "keep-last-plan").retention.as_ref().unwrap().to_resty().unwrap();
        assert_eq!(r.keep_last, Some(7));
        assert_eq!(r.keep_daily, None);
        assert_eq!(r.keep_weekly, None);
        assert_eq!(r.keep_monthly, None);
        assert_eq!(r.keep_yearly, None);
    }

    #[test]
    fn maps_time_bucketed_retention_dropping_hourly() {
        let cfg = parsed();
        let r = plan(&cfg, "bucketed-plan").retention.as_ref().unwrap().to_resty().unwrap();
        // `hourly: 24` has no Resty equivalent and is dropped; the rest carry over.
        assert_eq!(r.keep_last, Some(14));
        assert_eq!(r.keep_daily, Some(7));
        assert_eq!(r.keep_weekly, Some(4));
        assert_eq!(r.keep_monthly, Some(6));
        assert_eq!(r.keep_yearly, Some(2));
    }

    #[test]
    fn keep_all_retention_maps_to_none() {
        let cfg = parsed();
        // `policyKeepAll` is ignored, leaving no representable buckets.
        assert!(plan(&cfg, "disabled-plan").retention.as_ref().unwrap().to_resty().is_none());
    }

    #[test]
    fn merges_case_insensitive_excludes() {
        let cfg = parsed();
        let p = plan(&cfg, "bucketed-plan");
        let mut excludes = p.excludes.clone();
        excludes.extend(p.iexcludes.iter().cloned());
        assert_eq!(excludes, vec!["cache", "tmp", "Thumbs.db"]);
    }
}
