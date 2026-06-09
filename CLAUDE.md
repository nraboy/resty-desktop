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
| Settings persistence | `tauri-plugin-store` (`settings.json`) |
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
    types.ts                  # Shared TS types: Repository, Snapshot, FileEntry, ResticStats
    invoke.ts                 # Typed wrappers over tauri invoke()
  pages/
    RepositoriesPage.tsx      # Add/open/delete repos; triggers restic init for new repos; supports remote URLs (S3, SFTP, etc.)
    SnapshotsPage.tsx         # Table of snapshots; inline tag editor; delete with prune option
    BrowsePage.tsx            # File tree navigation inside a snapshot; per-entry restore
    BackupPlansPage.tsx       # List saved backup plans; run a plan immediately; delete plans
    BackupPlanEditPage.tsx    # Create/edit a backup plan (name, repo, paths, tags, excludes); planId="new" for creation
    SettingsPage.tsx          # Restic binary path override; install instructions

src-tauri/
  Cargo.toml
  tauri.conf.json
  src/
    main.rs                   # Calls restic_gui_lib::run()
    lib.rs                    # Tauri builder; registers all commands
    commands/
      mod.rs                  # shared get_restic_path() helper used by all command modules
      repo.rs                 # list_repos, add_repo, remove_repo, init_repo, get_repo_stats, get/set_restic_path
      snapshot.rs             # list_snapshots, delete_snapshot, tag_snapshot, run_backup, forget_by_plan
      browse.rs               # list_files, restore_path
      backup_plan.rs          # list_backup_plans, save_backup_plan, remove_backup_plan; plans stored in settings.json under "backup_plans" key
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
- Repos and settings are stored in `settings.json` via `tauri-plugin-store`.

## Adding a New Feature

1. Add a `#[tauri::command]` function in the appropriate `src-tauri/src/commands/*.rs` file.
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
