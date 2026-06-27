# Import / Export Application Data

## Context

Resty Desktop stores all of its configuration — repositories (with encrypted
passwords), backup plans, and schedules — in a local SQLite `app_data.db`,
encrypted under a device-local master key. There is currently no way to move
this configuration between installations (new machine, reinstall, second
device). This feature adds an **Export** (write selected config to a portable,
secret-encrypted file) and **Import** (read that file back, decrypt, and insert)
flow, surfaced in Settings.

### Decisions (from user)
- **Encryption scope:** only secret fields (repo passwords) are encrypted; the
  rest of the file is readable JSON.
- **Export passphrase:** required only when Repositories are included; otherwise
  optional/absent.
- **Selection:** checkboxes per category (Repositories / Backup Plans /
  Schedules). Dependencies auto-enforced.
- **Conflicts / IDs:** always **import as copies** — generate fresh UUIDs for
  every imported item and rewrite all references. Never overwrite or skip.
- **Paths:** imported verbatim, no validation. Import preview shows a warning
  that paths may need to be checked on this machine.
- **App settings & backup history:** excluded entirely from export.
- **Import flow:** preview/summary first, then explicit Confirm. Transactional.

## File format (`.resty` JSON)

```jsonc
{
  "app": "resty",
  "version": 1,
  "exportedAt": 1719500000,           // unix seconds
  "encryption": {                      // present only if repos included
    "kdf": "argon2id",
    "salt": "<base64>"                 // KDF salt for the export passphrase
  },
  "repositories": [
    { "name": "Home NAS", "path": "/Volumes/nas",
      "password": { "nonce": "<base64>", "ciphertext": "<base64>" } }
  ],
  "backupPlans": [
    { "name": "...", "repoRef": 0,     // index into repositories[] (see below)
      "paths": [...], "tags": [...], "excludes": [...],
      "retention": {...}|null, "limitUpload": null, "limitDownload": null }
  ],
  "schedules": [
    { "name": "...", "planRefs": [0,1], // indices into backupPlans[]
      "cronExpr": "...", "enabled": true }
  ]
}
```

**Reference strategy:** because we always mint fresh UUIDs on import, the file
stores cross-references as **array indices**, not UUIDs. `backupPlans[].repoRef`
indexes `repositories[]`; `schedules[].planRefs` index `backupPlans[]`. This
makes the file self-contained and import remapping trivial (no UUID matching).
Original IDs, `lastRunAt`/`nextRunAt`/`createdAt` are intentionally dropped.

**Dependency enforcement on export:** if a selected plan's repo isn't selected,
include it anyway (and likewise schedules pull in their plans, which pull in
their repos). The import file is always referentially complete.

## Backend (Rust)

### New dependency
Add `base64 = "0.22"` to `src-tauri/Cargo.toml` (for text-safe encoding of
nonce/ciphertext/salt). `serde`, `serde_json`, `zeroize` already present.
`dialog:default` already grants the save dialog — no capability change.

### New file: `src-tauri/src/commands/transfer.rs`
Mirror the existing module style (see `repo.rs`, `snapshot.rs`).

Serde structs for the bundle (`ExportBundle`, `ExportRepo`, `EncSecret`,
`ExportPlan`, `ExportSchedule`, `ExportEncryption`) with
`#[serde(rename_all = "camelCase")]`.

**`export_data`** command:
```rust
#[tauri::command]
pub async fn export_data(
    db: State<'_, AppDb>, master_key: State<'_, MasterKey>,
    repo_ids: Vec<String>, plan_ids: Vec<String>, schedule_ids: Vec<String>,
    export_password: Option<String>,
) -> Result<String, String>   // returns the JSON string
```
- `let key = master_key.get()?;`
- Expand selection to a referentially-complete set (add repos referenced by
  selected plans; add plans referenced by selected schedules; their repos too).
- If repos in final set is non-empty: require `export_password`; derive an
  export key with `crypto::derive_key(&pw, &salt)` using a fresh
  `crypto::random_bytes::<16>()` salt. For each repo:
  `db.get_full_repo(id, &key)` → `crypto::encrypt(&export_key, password.as_bytes())`
  → base64 the `(nonce, ciphertext)`. Zeroize the intermediate plaintext
  (`FullRepository` already `ZeroizeOnDrop`; zeroize any `String` copy).
- Build plans/schedules from `db.list_backup_plans()` / `db.list_schedules()`
  filtered to the expanded set, rewriting `repoId`/`planIds` into array indices.
- `serde_json::to_string_pretty(&bundle)`.

The frontend writes the returned string to disk via the save dialog +
`@tauri-apps/plugin-fs` `writeTextFile` (add `fs:allow-write-text-file` to
capabilities) — **or** simpler: pass the chosen path into a second tiny command
that does `std::fs::write`. **Chosen approach:** have `export_data` take an
`out_path: String` and write the file itself with `std::fs::write`, returning
counts. Keeps secrets out of the JS layer entirely. Signature becomes:
```rust
... out_path: String) -> Result<ExportSummary, String>  // { repos, plans, schedules }
```

**`preview_import`** command:
```rust
#[tauri::command]
pub fn preview_import(file_path: String, export_password: Option<String>)
    -> Result<ImportPreview, String>
```
- Read + `serde_json::from_str`. Validate `app == "resty"` and `version == 1`
  (reject unknown versions with a clear message).
- If `encryption` present: require password; attempt to decrypt the **first**
  repo secret to verify the passphrase early; on failure return the existing
  `"Decryption failed — incorrect master password"`-style message (reword for
  export context). Do **not** return decrypted secrets to the frontend.
- Return `ImportPreview { repos: u32, plans: u32, schedules: u32,
  local_repo_paths: Vec<String>, requires_password: bool }` so the UI can show
  counts + the path-review warning.

**`import_data`** command:
```rust
#[tauri::command]
pub fn import_data(
    db: State<'_, AppDb>, master_key: State<'_, MasterKey>,
    file_path: String, export_password: Option<String>,
) -> Result<ImportSummary, String>
```
- `let key = master_key.get()?;` Re-read + parse the file (don't trust anything
  cached from preview).
- Derive export key from salt+password if `encryption` present.
- **New `AppDb::import_bundle` method** doing the whole insert in **one SQLite
  transaction** (mirror `rotate_master_key`'s transaction style, lines ~430-476
  of `cache.rs`):
  - For each repo: `crypto::decrypt(export_key, nonce, ct)` → re-encrypt with
    the local master `key` via `crypto::encrypt` → `INSERT` with a fresh
    `uuid` (generate Rust-side with the `uuid` crate, or accept fresh IDs from
    the command layer). Build `index → new_repo_id` map. Zeroize each decrypted
    password.
  - For each plan: fresh UUID, resolve `repoRef` via the map, `save_backup_plan`.
    Build `index → new_plan_id` map.
  - For each schedule: fresh UUID, resolve `planRefs` via the map, set
    `lastRunAt/nextRunAt = null`, `createdAt = now`, recompute `next_run_at`
    from cron (reuse the schedule-save path that already computes this). Insert.
  - Commit (rollback on any error → all-or-nothing).
- Name de-duplication: if an imported name collides with an existing one,
  append `" (imported)"` to keep lists readable. Apply for all three types.

UUID generation: add the `uuid` crate (`uuid = { version = "1", features =
["v4"] }`) OR generate IDs on the frontend (`crypto.randomUUID()`, the existing
convention per `RepositoriesPage.tsx:162`) and pass arrays in. **Chosen:**
generate Rust-side with `uuid` for atomicity (the whole import is one backend
call) — add the crate.

### Register commands
Add `export_data`, `preview_import`, `import_data` to the `invoke_handler!`
macro in `src-tauri/src/lib.rs`, and `mod transfer;` where the other command
modules are declared.

## Frontend (React/TS)

### `src/lib/types.ts`
Add `ImportPreview` and `ImportSummary` / `ExportSummary` types matching the
Rust returns.

### `src/lib/invoke.ts`
New `// ── import / export ──` section:
```ts
export const exportData = (repoIds: string[], planIds: string[],
  scheduleIds: string[], outPath: string, exportPassword?: string)
  : Promise<ExportSummary> =>
  invoke("export_data", { repoIds, planIds, scheduleIds, outPath,
    exportPassword: exportPassword ?? null });
export const previewImport = (filePath: string, exportPassword?: string)
  : Promise<ImportPreview> =>
  invoke("preview_import", { filePath, exportPassword: exportPassword ?? null });
export const importData = (filePath: string, exportPassword?: string)
  : Promise<ImportSummary> =>
  invoke("import_data", { filePath, exportPassword: exportPassword ?? null });
```

### `src/pages/SettingsPage.tsx`
Add one new card (`mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5`,
matching the existing card shell at line ~218) titled **"Import & Export"**,
with two `<Button>`s: **Export…** and **Import…**, each opening a `<Modal>`.
Reuse the existing modal/state idioms (the prune-all modal at lines ~131-181 /
473-542 is the closest template). Fetch the lists for the export picker via the
existing `listRepos` / `listBackupPlans` / `listSchedules` invoke wrappers.

**Export modal:**
- Three checkbox groups (Repositories / Backup Plans / Schedules), each listing
  items by name. Auto-check dependencies visually when a dependent is checked
  (or just enforce on the backend and note it). "Select all" per group.
- Export passphrase + confirm-passphrase `<Input type="password">`, shown/
  required only when ≥1 repo is selected.
- On submit: `save({ defaultPath: "resty-export.resty", filters:[{name:"Resty
  Export", extensions:["resty","json"]}] })` from `@tauri-apps/plugin-dialog`
  (new `save` import — pattern mirrors the existing `open` usage at
  `SettingsPage.tsx:372`). If a path is returned, call `exportData(...)`, show
  the returned counts as a success state.

**Import modal:**
- Step 1: "Choose file…" → `open({ multiple:false, filters:[...] })`.
- Step 2: if the preview call reports `requiresPassword`, show a passphrase
  `<Input type="password">`. Call `previewImport`.
- Step 3: show summary ("Will import N repositories, M plans, K schedules as new
  copies") + a warning box: *"Repository and backup paths are imported as-is and
  may not exist on this machine — review them after importing."* List
  `localRepoPaths` if any.
- Step 4: **Confirm Import** → `importData(...)`, show result summary. Surface
  decryption errors (wrong passphrase) inline.

No streaming progress needed (import/export are fast, synchronous DB ops) — use
a simple `loading` flag on the Confirm/Export buttons rather than the
event-listener pattern.

## Critical files
- **New:** `src-tauri/src/commands/transfer.rs`
- `src-tauri/src/commands/cache.rs` — add `AppDb::import_bundle` (transactional);
  reuse `get_full_repo`, `list_backup_plans`, `list_schedules`, the
  `rotate_master_key` transaction pattern, and `save_backup_plan`/`save_schedule`.
- `src-tauri/src/commands/crypto.rs` — reuse `derive_key`, `encrypt`, `decrypt`,
  `random_bytes` as-is (no changes).
- `src-tauri/src/lib.rs` — `mod transfer;` + register 3 commands.
- `src-tauri/Cargo.toml` — add `base64`, `uuid`.
- `src/lib/invoke.ts`, `src/lib/types.ts`, `src/pages/SettingsPage.tsx`.

## Verification
1. `npm run tauri dev`.
2. With a few repos, plans, and schedules configured: Settings → Export…, select
   all, set a passphrase, save `test.resty`. Open the file — confirm plans/
   schedules are readable JSON and repo passwords are base64 `{nonce,ciphertext}`,
   never plaintext.
3. Export with only Backup Plans checked — confirm the passphrase field is hidden
   and the bundle still includes the referenced repos (dependency completeness).
4. Import `test.resty` into the same install: verify preview counts, the path
   warning shows, and after Confirm every item appears with a fresh UUID and a
   `" (imported)"` suffix on name collisions; references intact (plans point at
   the new repo copies; schedules at the new plan copies).
5. Wrong passphrase on import → clear error, no partial insert (verify counts
   unchanged).
6. Round-trip on a second machine / fresh `reset_app`: import, then open a
   repo's snapshots to confirm the re-encrypted password decrypts under the new
   master key.
7. Edit a repo password back-and-forth is unaffected; run `cargo build` clean.
