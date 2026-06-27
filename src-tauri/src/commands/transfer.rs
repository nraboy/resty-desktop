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
        serde_json::from_str(&raw).map_err(|_| "This is not a valid Resty export file.".to_string())?;
    if bundle.app != "resty" {
        return Err("This is not a valid Resty export file.".to_string());
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
            let (nonce, ct) = crypto::encrypt(export_key, full.password.as_bytes())?;
            repositories.push(ExportRepo {
                id: r.id.clone(),
                name: r.name.clone(),
                path: r.path.clone(),
                password: EncSecret {
                    nonce: B64.encode(nonce),
                    ciphertext: B64.encode(ct),
                },
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
            if let Some(first) = bundle.repositories.first() {
                let mut plaintext = decrypt_secret(&key, &first.password)?;
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
            enabled: s.enabled,
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
