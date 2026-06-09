# Restic GUI Client

A cross-platform desktop client for the Restic CLI backup tool.

## Technical Details and Features

- Uses Tauri v2 as the cross-platform desktop framework.
- Acts as a wrapper to the already established Restic CLI application, installed separately.
- Should be able to create repositories, add folders to snapshots, view snapshots, view the files within snapshots, tag snapshots, delete snapshots, etc.

## Stack

| Layer | Choice |
|---|---|
| Desktop shell | Tauri v2 |
| Frontend | React 19 + TypeScript |
| Styling | Tailwind CSS v3 |
| Build tool | Vite |
| State management | URL-based nav (no global store) |
| Routing | React Router v6 |
| Rust backend | Tauri v2 `#[tauri::command]` |
| Settings persistence | SQLite (`app_data.db`) via `AppDb`; `tauri-plugin-store` kept only for legacy migration |
| File picker | `tauri-plugin-dialog` |
| Shell plugin | `tauri-plugin-shell` (registered but not exposed to frontend) |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Restic integration | `std::process::Command` with `--json` flag |

## Project Structure

```
src/
  App.tsx                     # Router + layout shell
  main.tsx                    # React entry point
  index.css                   # Tailwind directives + global styles
  components/
    Button.tsx                # Reusable button (primary/secondary/danger/ghost variants)
    EmptyState.tsx            # Empty list placeholder
    Input.tsx                 # Labeled input with error state
    Modal.tsx                 # Overlay modal dialog
    Sidebar.tsx               # Left nav with active repo indicator
  lib/
    types.ts                  # Shared TS types: Repository, Snapshot, FileEntry, ResticStats; isRemoteRepo() helper
    invoke.ts                 # Typed wrappers over tauri invoke()
  pages/
    RepositoriesPage.tsx      # Add/open/delete repos; triggers restic init for new repos; supports remote URLs (S3, SFTP, etc.)
    SnapshotsPage.tsx         # Table of snapshots; inline tag editor; delete with prune option; stale-while-revalidate cache pattern
    BrowsePage.tsx            # File tree navigation inside a snapshot; per-entry restore; breadcrumb nav
    BackupPlansPage.tsx       # List saved backup plans; run a plan immediately; delete plans
    BackupPlanEditPage.tsx    # Create/edit a backup plan (name, repo, paths, tags, excludes); planId="new" for creation
    SettingsPage.tsx          # Restic binary path override; install instructions

src-tauri/
  Cargo.toml
  tauri.conf.json
  src/
    main.rs                   # Calls restic_gui_lib::run()
    lib.rs                    # Tauri builder; registers all commands; opens app_data.db, initialises schema, manages AppDb + MasterKey as Tauri state
    commands/
      mod.rs                  # shared get_restic_path() helper used by all command modules
      auth.rs                 # is_app_setup, setup_master_password, unlock_app, lock_app,
                              #   change_master_password, reset_app; migrates legacy settings.json on first setup
      crypto.rs               # Argon2id key derivation, AES-GCM encrypt/decrypt helpers
      repo.rs                 # list_repos, add_repo, remove_repo, init_repo, rename_repo,
                              #   test_repo_connection, get_repo_stats, refresh_repo_stats, get/set_restic_path
      snapshot.rs             # list_snapshots, refresh_snapshots, delete_snapshot, tag_snapshot,
                              #   run_backup, forget_by_plan, unlock_repo
      browse.rs               # list_files, restore_path
      backup_plan.rs          # list_backup_plans, save_backup_plan, remove_backup_plan; plans stored in SQLite
      cache.rs                # AppDb (unified SQLite state); MasterKey (in-memory); Repository, FullRepository,
                              #   BackupPlan, RetentionPolicy types; clear_browse_cache command
```

## Routes

| Path | Page |
|---|---|
| `/` | RepositoriesPage |
| `/snapshots/:repoId` | SnapshotsPage |
| `/snapshots/:repoId/:snapshotId/browse` | BrowsePage |
| `/backup-plans` | BackupPlansPage |
| `/backup-plans/:planId` | BackupPlanEditPage (`planId="new"` for creation) |
| `/settings` | SettingsPage |

## Restic Integration

- Restic binary path is user-configurable; defaults to `restic` on `$PATH`.
- All commands set both `RESTIC_REPOSITORY` and `RESTIC_PASSWORD` env vars — never pass either in process args.
- Structured output parsed via `restic --json`; `serde_json` deserializes responses into typed Rust structs.
- `restic ls --json` outputs NDJSON (one JSON object per line); the first line is a snapshot summary and is skipped; subsequent lines are `FileEntry` objects filtered to direct children only.
- `run_backup` returns the raw restic JSON stdout as a `String` (not deserialized).
- Repos, backup plans, and app settings are stored in SQLite (`app_data.db`) via `AppDb`. Repo passwords are AES-GCM encrypted with the master key before storage.

## Security Architecture

- App requires a **master password** set on first launch (`setup_master_password`). Subsequent launches call `unlock_app` to load the key into memory.
- Master password is never stored. Instead: Argon2id derives a 32-byte key from password + random salt; AES-GCM encrypts a known verification plaintext; the salt + nonce + ciphertext are stored in the `master_key` table.
- All repo passwords are encrypted with the master key (AES-GCM) and stored in the `repositories` table. They are decrypted on-demand via `db.get_full_repo(&repo_id, &key)`.
- `MasterKey` is an in-memory `Mutex<Option<[u8; 32]>>` managed as `tauri::State`. It is `None` when locked; any command that calls restic will fail with "App is locked" until `unlock_app` succeeds.
- `change_master_password` re-derives a new key and re-encrypts all repo passwords atomically in a SQLite transaction.
- `reset_app` wipes all SQLite tables and clears the in-memory key, returning to first-launch state.
- On first `setup_master_password`, any existing `settings.json` data (repos, backup plans, restic path) is migrated into SQLite and encrypted.

## Persistence Layer

- Single SQLite database (`app_data.db`) in the Tauri app data directory, opened at startup and managed as `tauri::State<AppDb>`.
- Tables: `master_key`, `repositories` (encrypted passwords), `backup_plans`, `app_settings`, `snapshots_cache`, `browse_cache`, `repo_stats_cache`.
- `tauri-plugin-store` (`settings.json`) is still registered but only used by `migrate_from_settings_json` during first-time setup. All new reads/writes go through `AppDb`.

## Caching Layer

- Three cache tables in `app_data.db`: `snapshots_cache` (per-repo snapshot list), `browse_cache` (per-snapshot directory listings keyed by path), `repo_stats_cache` (per-repo stats).
- `list_snapshots` — returns from cache only (fast, no restic call). `refresh_snapshots` — calls restic and updates cache.
- `SnapshotsPage` uses a stale-while-revalidate pattern: serve cache immediately, then fire `refresh_snapshots` in the background for local repos. Remote repos skip the background refresh to avoid unnecessary network calls.
- After `run_backup` succeeds: parse the new `snapshot_id` from the restic NDJSON summary line, fetch that single snapshot's metadata (`restic snapshots --json <id>`), and prepend it to the cached list — no full re-fetch needed.
- After `forget_by_plan` succeeds: run `restic snapshots --json` to repopulate the snapshot cache with the post-prune list.
- Stats cache (`get_repo_stats` / `refresh_repo_stats`): for remote repos, stats are evicted after backup/forget rather than auto-repopulated, since `restic stats` reads pack indexes which can be large on remote storage.
- `clear_browse_cache` command wipes all three cache tables (exposed to frontend for manual cache clearing).

## Adding a New Feature

1. Add a `#[tauri::command]` function in the appropriate `src-tauri/src/commands/*.rs` file.
   - If the command needs to call restic, accept `State<'_, AppDb>` and `State<'_, MasterKey>`, call `master_key.get()?` to obtain the key, then `db.get_full_repo(&repo_id, &key)?` to retrieve decrypted credentials.
2. Register it in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.
3. Add a typed wrapper in `src/lib/invoke.ts`.
4. Consume it from a page or hook.

## Running the App

Rust must be installed first (it is not bundled):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# restart terminal, then:
npm install
npm run tauri dev
```

Build a distributable:

```bash
npm run tauri build
```
