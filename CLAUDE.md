# Resty Desktop

A cross-platform desktop client for the Restic CLI backup tool.

This file is the always-loaded index. Deep rationale for each area lives in `docs/*.md` — read
the linked doc before touching that area; each doc opens with a "read this before" line stating
its scope. See **Where the detail lives** and **Settled decisions** below.

## Stack

| Layer | Choice |
|---|---|
| Desktop shell | Tauri v2 |
| Frontend | React 19 + TypeScript |
| Styling | Tailwind CSS v3 + CSS custom properties for theming |
| Build tool | Vite |
| State management | URL-based nav (no global store) |
| Routing | React Router v6 |
| Rust backend | Tauri v2 `#[tauri::command]` |
| Settings persistence | SQLite (`app_data.db`) via `AppDb` |
| File picker | `tauri-plugin-dialog` (xdg-desktop-portal backend on Linux, not GTK; see docs/decisions.md) |
| Shell plugin | `tauri-plugin-shell` (registered but not exposed to frontend) |
| Memory safety | `zeroize` crate — `MasterKey`/`FullRepository` zeroize sensitive bytes on drop/replace; see docs/data.md |
| Notifications | `tauri-plugin-notification` — shown on backup success/failure |
| Single-instance | `tauri-plugin-single-instance` — prevents multiple processes; focuses existing window on relaunch |
| Launch at login | `tauri-plugin-autostart`, OS-native entry, gated on the tray setting; see docs/decisions.md |
| Auto-unlock | `keyring` 3 (macOS/Windows only), opt-in, stores the *derived* master key; see docs/data.md |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Restic integration | `std::process::Command` with `--json`; see docs/restic.md |

## Project Structure

Paths and one-line summaries only — full per-file behavior is in docs/frontend.md (`src/`) and
docs/backend.md (`src-tauri/`).

```
src/
  App.tsx               # Router + layout shell; auth state machine (loading/setup/locked/unlocked/updateNotice), tries auto-unlock before rendering the password screen — see docs/data.md
  main.tsx              # React entry; suppresses context menu globally
  index.css             # Tailwind directives + global styles
  components/
    Button.tsx            # primary/secondary/danger/ghost variants
    ContextMenu.tsx       # Portal-rendered right-click menu; auto-nudges onto screen
    EmptyState.tsx        # Empty list placeholder
    ImportExportCard.tsx  # Settings card: export/import bundle + Backrest config.json — see docs/data.md
    Input.tsx             # Labeled input with error state; optional inline clear
    Modal.tsx             # Overlay modal dialog
    ProgressBar.tsx       # Determinate/indeterminate progress bar, shared across modals
    Sidebar.tsx           # Left nav with app icon + repo indicator
    ActivityPanel.tsx     # Right-side drawer surfacing background activity (indexing, scheduler backups, stats, mirrors) — see docs/concurrency.md
  lib/
    types.ts              # Shared TS types (Repository, Snapshot, BackupPlan, etc.); isRemoteRepo() helper
    backends.ts           # detectBackend + credential hints for S3/B2/REST — see docs/restic.md
    invoke.ts             # Typed wrappers over tauri invoke()
    activity.tsx          # ActivityProvider: indexing, scheduler backups, stats, mirrors via the task event bus — see docs/concurrency.md
    format.ts             # formatBytes, formatSize, formatDate, formatDuration, etc.
    config.ts             # MIN_RESTIC_MAJOR/MINOR constants for version warning
    utils.ts              # needsFullDiskAccess(paths) — macOS protected-path check
    theme.tsx             # ThemeProvider + useTheme(); persists to localStorage
    cron.ts               # parseCronToSimple/buildCronExpr — pure cron helpers, unit-tested
    difftree.ts           # computeChildren/toSegments — pure tree-building for DiffPage, unit-tested
  pages/
    AuthPage.tsx          # Master password setup (first launch) and unlock screen
    RepositoriesPage.tsx  # Add/open/delete/edit repos, remote URL + credentials, read-only flag, stats refresh, mirror, prune, check, Index All — see docs/frontend.md
    SnapshotsPage.tsx     # Snapshot table; stale-while-revalidate cache; restore, copy, tag, delete, per-snapshot index — see docs/frontend.md
    BrowsePage.tsx        # File tree inside a snapshot; restore, tag management, search entry point — see docs/frontend.md
    SearchPage.tsx        # Full-text file search within one snapshot; index state machine — see docs/frontend.md
    RepoSearchPage.tsx    # Full-text file search across every indexed snapshot in a repo; Index All batch — see docs/frontend.md
    DiffPage.tsx          # Diff viewer between two snapshots; client-side tree, restore from diff
    BackupPlansPage.tsx   # List/run/delete plans; backup modal with progress; auto-applies retention — see docs/frontend.md
    BackupPlanEditPage.tsx # Create/edit plan: paths, tags, excludes, retention, bandwidth limits — see docs/frontend.md
    SchedulesPage.tsx     # List schedules; toggle/delete/run; read-only-repo warnings
    ScheduleEditPage.tsx  # Create/edit schedule (cron expr, backup plans); read-only-repo badges
    LogsPage.tsx          # Backup history log; paginated; expandable error rows
    SettingsPage.tsx      # Theme, tray, launch-at-login, auto-unlock, restic path, prune all, import/export, cache — see docs/frontend.md and docs/decisions.md
src-tauri/
  src/
    main.rs               # Calls restic_gui_lib::run()
    lib.rs                # Tauri builder; registers commands; manages app state; native menu bar; tray — see docs/concurrency.md
    commands/
      mod.rs                # get_restic_path(); NoConsole trait for Finder-launched PATH
      auth.rs               # Setup/unlock/lock master password; auto-unlock — see docs/data.md
      crypto.rs             # Argon2id key derivation, AES-GCM encrypt/decrypt
      keychain.rs           # Auto-unlock's OS credential-manager access — see docs/data.md
      backends.rs           # Backend credential registry (S3/B2/REST) — see docs/restic.md
      repo.rs               # Repo CRUD, stats, restic path/version, prune, FDA checks — see docs/restic.md and docs/concurrency.md
      repo_locks.rs         # RepoLocks: per-repo shared/exclusive lock registry — see docs/concurrency.md
      snapshot.rs           # List/delete/tag snapshots; execute_backup; copy/mirror; retention — see docs/restic.md and docs/concurrency.md
      browse.rs             # File listing, restore, indexing (single + batch) — see docs/concurrency.md and docs/decisions.md
      backup_plan.rs        # List/save/remove backup plans
      schedule.rs           # List/save/remove/toggle schedules; run_schedule_now
      transfer.rs           # Export/import bundle + Backrest config.json import — see docs/data.md
      cache.rs              # AppDb (SQLite state), MasterKey, operation handles — see docs/concurrency.md
  cache_warmer.rs       # Background sweep: snapshot refresh + auto-indexing — see docs/concurrency.md
  scheduler.rs          # Background tick runs due schedules via execute_backup — see docs/concurrency.md and docs/decisions.md
```

## Routes

| Path | Page |
|---|---|
| `/` | RepositoriesPage |
| `/snapshots/:repoId` | SnapshotsPage |
| `/snapshots/:repoId/search` | RepoSearchPage |
| `/snapshots/:repoId/:snapshotId/browse` | BrowsePage |
| `/snapshots/:repoId/:snapshotId/search` | SearchPage |
| `/snapshots/:repoId/diff/:snapshotA/:snapshotB` | DiffPage |
| `/backup-plans` | BackupPlansPage |
| `/backup-plans/:planId` | BackupPlanEditPage (`planId="new"` for creation) |
| `/schedules` | SchedulesPage |
| `/schedules/:scheduleId` | ScheduleEditPage (`scheduleId="new"` for creation) |
| `/logs` | LogsPage |
| `/settings` | SettingsPage |


## Where the detail lives

| Task | Read |
|---|---|
| Locks, `RepoLocks`, cancellable ops, the `task` event bus | `docs/concurrency.md` |
| Adding/changing a restic subprocess call, backend credentials, read-only repos | `docs/restic.md` |
| Schema/migrations, encryption, caching, import/export | `docs/data.md` |
| Page/component behavior, theming detail | `docs/frontend.md` |
| Tauri command internals not covered by the three docs above | `docs/backend.md` |
| Before "fixing" something that looks like a bug or inefficiency | `docs/decisions.md` (check the index below first) |

## Restic Integration

Full detail (env-var policy, streaming vs. one-shot calls, `mirror_repo`'s queueing, backend
credentials incl. REST, stats-cache policy): **docs/restic.md**.

Retain always: restic's binary path is user-configurable (defaults to `restic` on `$PATH`); every
command sets `RESTIC_REPOSITORY`/`RESTIC_PASSWORD` as **env vars, never process args**; every
restic `Command` must set `.stdin(Stdio::null())` (Windows Scoop-shim spawn failures otherwise)
alongside `.no_console()`; an `async fn` `#[tauri::command]` must **never** call
`std::process::Command` inline — always `spawn_blocking` (streaming) or `run_restic_blocking`
(one-shot) — or it blocks a shared tokio worker and starves every other async command plus the
`AppDb` lock. A repo can be marked **read-only** (`--no-lock` on every read op via
`apply_repo_flags`); every write op instead calls `ensure_writable` and refuses outright. A stats
refresh (`fetch_and_cache_stats`) runs restic **twice** — default mode for the restore-size figures,
then `--mode raw-data` for the on-disk stored size (`ResticStats.raw_size`) — with the second call's
failure deliberately non-fatal to the refresh; see docs/restic.md.

## Concurrency: Per-Repository Lock Registry

Full detail (which commands take which guard, retry-on-"already locked" policy, the two prune
race fixes): **docs/concurrency.md**.

Retain always: `RepoLocks` (`repo_locks.rs`) is an in-memory per-repo-**path** shared/exclusive
lock registry — shared-lock ops (`ReadGuard`) never block; exclusive-lock ops (`WriteGuard`) poll
until zero readers, **with no timeout or force-claim** (a 15s force-claim was tried and caused a
confirmed regression — don't reintroduce one). Readers deliberately never wait for writers; **do
not "complete" the registry by making them** — that would let a slow exclusive op on a large/remote
repo hang every snapshot listing and stats call, a worse regression than the rare collision this
registry exists to prevent. `RepoLocks` only coordinates this app's own operations, not an external
restic/cron process — the retry-on-"already locked" logic remains the backstop for that.

## Operation Event Bus

Full detail (six frontend consumers, why `operationId` not `repoId`, the mirror/index-batch
per-run registries): **docs/concurrency.md**.

Retain always: `tasks.rs` defines a uniform `task` event (`TaskEvent`) layered **on top of**,
never instead of, existing detailed feeds (`backup:progress`, etc.). Envelope: `operationId`
(unique per operation instance — the correlation key, since `repoId` alone can't distinguish
concurrent operations), `kind`, `phase` (`started`|`progress`|`cancelling`|`cancelled`|
`finished`|`failed`), `repoId`, `targetId`, `origin`, `progress`, `error`, `at`. Every
restic-shelling operation is wired via `OperationCtx`, except pure-DB reads (`list_snapshots`,
`get_repo_stats`) and the continuous 60s `cache_warmer` snapshot-refresh tick.

## Security Architecture

Full detail (rotation transaction, auto-unlock keychain design and trade-offs): **docs/data.md**.

Retain always: master password → Argon2id → 32-byte key; AES-GCM encrypts a verification
plaintext; **the password itself is never stored**. `MasterKey` is `Mutex<Option<[u8; 32]>>` —
`None` when locked, every restic command fails with "App is locked". Repo passwords and backend
credentials are AES-GCM-encrypted under the master key. Auto-unlock (opt-in, default off,
macOS/Windows only) stores the *derived* key, never the password, in the OS credential manager.

## Persistence & Caching

Full detail (schema, migrations, stale-while-revalidate patterns, cache warmer): **docs/data.md**.

Retain always: single SQLite `app_data.db` via `AppDb`, one shared `Mutex<Connection>` — a slow
synchronous query on a core async-runtime thread starves every other `AppDb`-touching command.
Any new command doing DB work slow enough to notice should be `async fn` + `spawn_blocking`.

## Settled decisions — do not re-flag without reading `docs/decisions.md`

These were deliberately kept as-is after prior audits raised them. Read the linked entry before
proposing a change — several are pinned by a named test or reference a confirmed regression.

- Backend credentials use their own nonce/ciphertext columns, not folded into the password blob
- `apply_backend_env` filters reserved credential keys itself, not just `validate_credentials`
- `validate_credentials` deliberately allows arbitrary keys for `BackendKind::Rest`
- `"rest:"` stays listed in `REMOTE_PREFIXES` even though a dedicated `detect_kind` arm exists
- Sync `#[tauri::command]`s are not wrapped in `spawn_blocking` (Tauri already offloads them)
- `scheduler.rs`/`run_schedule_now` call sync `apply_retention` directly, not via `spawn_blocking`
- `list_snapshots`, `get_snapshot_index_status`, `get_repo_stats` don't emit on the `task` bus
- `get_repo_stats` is cache-only and must never fall through to a live `restic stats` call
- `browse_cache_files.parent_path` duplicates a prefix of `path` on purpose (index speed)
- Stored repo size (`ResticStats.raw_size`) comes from `restic stats --mode raw-data`, not a `du`/filesystem walk (works for remote repos too); a failed raw-data call is non-fatal to the refresh
- File search uses `LIKE '%query%'` (leading wildcard, no index) — accepted, not an oversight
- `cached_at` columns are written but not read yet — kept for a future TTL feature
- `panic = "abort"` is deliberately not set in the release profile
- Known, deferred frontend duplication (search/index pattern, FileIcon, index-batch UI, etc.)
- Backup progress bars are non-monotonic by design (restic's own `percent_done` behavior)
- `IndexHandle::gate` must stay one app-wide mutex — never split per-batch or per-repo
- `gpu_compat::apply()`'s NVIDIA+Wayland workaround is gated, not applied unconditionally
- Linux file dialogs use `tauri-plugin-dialog`'s `xdg-portal` feature, not the default `gtk3` one — `rfd` silently reverts to GTK if `gtk3` is enabled anywhere in the dependency graph, so don't add it back
- Launch-at-login has no `app_settings` row (OS entry is the sole source of truth)
- Auto-unlock toggle is deliberately not gated on launch-at-login or the tray setting
- `.app_name("resty-desktop")` on the autostart builder must not be dropped
- A login launch shows the unlock screen; it never launches hidden into the tray
- `handleTrayToggle` clears launch-at-login on *enabling* the tray, not only on disabling
- `set_launch_at_login`'s disable path checks `is_enabled()` first (Windows idempotency)
- `reset_app` also best-effort clears the OS autostart entry, not just DB tables
- The Windows `Run` registry value `auto-launch` writes is unquoted — not fixable from app code
- `react-router-dom` stays on 6.x — its two audit advisories are unreachable in this app

## Import / Export

Full detail (bundle schema, dangling-reference handling, Backrest one-way import mapping and
lossy fields): **docs/data.md**.

Retain always: `transfer.rs` exports a portable `.json` bundle — repo passwords/credentials
encrypted under a user-supplied export passphrase, everything else plaintext and hand-editable.
Import always creates **fresh copies** (new UUIDs, refs remapped, `" (imported)"` dedup);
imported schedules are always disabled. `preview_backrest_import`/`import_backrest_config`
one-way-import a Backrest `config.json`; lossy by design (hooks, flags/env, bandwidth limits, etc.
are silently dropped — see docs/data.md for the full list).

## Adding a New Feature

1. Add `#[tauri::command]` in the appropriate `src-tauri/src/commands/*.rs` file. For restic calls: accept `State<'_, AppDb>` + `State<'_, MasterKey>`, call `master_key.get()?`, then `db.get_full_repo(&repo_id, &key)?`.
2. Register in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.
3. Add a typed wrapper in `src/lib/invoke.ts`.
4. Consume from a page.

## Theming

Three modes: Dark (default), Light, System. Stored in `localStorage`; applied as `dark`/`light`/`system` class on `<html>`. All theme-sensitive colors route through CSS custom properties in `src/index.css`, extended in `tailwind.config.js` (`gray.50–950`, `blue/green/red.300/400/700/900`, `amber.300/400/500/700/900`).

**Adding a themed color:** add `--tw-<color>-<shade>` to both `:root` and `html.light` (and the
`system` media-query block, kept identical to `light`) in `src/index.css`, extend
`tailwind.config.js`, then **verify contrast in light mode, not just that it compiles** — a shade
left out of the light pair silently falls through to Tailwind's dark-tuned default in *every*
theme (this is exactly how `amber-400`/`amber-500` shipped invisible on a white background; fixed
by reusing an already-corrected accent value rather than tuning a new one). Full worked example in
docs/frontend.md.

### Hardcoded colors to avoid
- `text-white` on gray backgrounds → use `text-gray-50` (remaps to near-black in light mode).
- `hover:text-white` on interactive elements → use `hover:text-gray-50`.
- `bg-red-700` for buttons → theme-mapped, becomes pastel pink in light mode. Use `bg-red-600 hover:bg-red-800`.
- Colors outside the extended set (`blue-500/600`, `red-500/6/8`, `yellow-*`) are NOT theme-mapped — intentional for colored-background elements like primary/danger buttons where white text is always on a dark surface, where the surface itself (not the page background) sets the contrast context.
- Amber/red/green/blue text used **without** a colored box behind it (a bare warning line, an
  inline status label) must use a mapped shade (`amber-300/400/500`, `red-300/400`, `green-300/400`,
  `blue-300/400`) — never an unmapped shade like `amber-600` or `red-500` — since that text sits
  directly on the page background, which flips between near-black and white across themes.


## Versioning

`src-tauri/tauri.conf.json`'s `version` field is the **only** version that matters — it's a literal
semver string, not a path, so Tauri reads it directly and never falls back to `package.json` (per
`@tauri-apps/cli`'s own config schema, that fallback only applies when `version` is set to a path
pointing at a `package.json` file, or omitted entirely, in which case it falls back to
`Cargo.toml`). The in-app version shown in `Sidebar.tsx` comes from `@tauri-apps/api/app`'s
`getVersion()`, which also resolves from `tauri.conf.json`. On a release bump, only
`tauri.conf.json` needs to change.

`package.json` and `package-lock.json` deliberately carry **no** `version` field — there's nothing
in the toolchain or CI that reads it (confirmed: neither workflow in `.github/workflows/` nor any
frontend/backend code references it), so there's nothing to keep in sync. Don't add one back.

`src-tauri/Cargo.toml`'s `version` is similarly deliberately left at `0.0.0` — that crate version is
unused (Tauri does not read it for the app version), and `0.0.0` signals "not the source of truth"
to avoid confusion; do not bump it.

## Build Profile

`src-tauri/Cargo.toml` sets `[profile.release]`: `strip = true`, `lto = true`, `codegen-units = 1` — a smaller/faster release binary at the cost of longer compile time (accepted; CI/local dev builds are unaffected since this only applies to `--release`). `opt-level` is left at the release default (`3`). `panic = "abort"` is deliberately **not** set — see "Settled decisions" above.


## Linux GPU Compatibility

`src-tauri/src/gpu_compat.rs` works around a WebKitGTK/NVIDIA/Wayland crash by setting env vars
before any GTK/WebKit init, but **only** when both an NVIDIA kernel module and a Wayland session
are detected — see docs/decisions.md for the full gating rationale (an unconditional fix would
slow down every other platform combination) and why `WEBKIT_DISABLE_COMPOSITING_MODE` is
deliberately not included. `RESTY_DISABLE_GPU_WORKAROUND` opts out without a rebuild.

## Releases

`.github/workflows/release.yml` — triggered by `v*` tag; builds on ubuntu-22.04, macos-latest, windows-latest via `tauri-apps/tauri-action@v0`; creates a draft GitHub Release. Annotated tag message becomes release body. Requires `permissions: contents: write`. Skipped on non-GitHub CI (`github.server_url` check).

Pre-built macOS binaries are not notarized: `sudo xattr -rd com.apple.quarantine /Applications/Resty\ Desktop.app`.

To cut a release, use `/tag` then:
```bash
git push origin main
git push origin v0.0.X
```

## Testing

- Frontend tests use **Vitest**; test files live alongside source as `src/lib/*.test.ts`.
- Rust unit tests use `#[cfg(test)]` modules in `scheduler.rs`, `cache_warmer.rs`, and `commands/{auth,backends,cache,crypto,repo,repo_locks,snapshot,schedule,transfer,browse}.rs`.
- CI (`.github/workflows/test.yml`) runs on every push that isn't a `v*` tag and on PRs.

```bash
npm run typecheck   # tsc --noEmit (tsconfig has strict + noUnusedLocals/Parameters)
npm run lint         # eslint src (react-hooks rules only, see below)
npm run lint:rust    # cargo clippy --all-targets -- -D warnings
npm run lint:all     # both of the above
npm run test:vite   # frontend tests only
npm run test:rust   # Rust tests only (cargo test)
npm run test:all    # both
```

Linting is deliberately narrow and **is wired into CI** — `.github/workflows/test.yml` runs
`npm run lint:all` on `ubuntu-22.04` alongside typecheck and both test suites, so a clippy
warning does fail the build; it's not merely a local-only gate. `eslint.config.js`
enables only `eslint-plugin-react-hooks` (`rules-of-hooks` + `exhaustive-deps`) — no
`typescript-eslint` rule sets, no stylistic rules — because `npm run typecheck` already covers
type errors and stylistic linting adds churn without preventing the regressions this project
actually sees. `npm run lint:rust` runs `cargo clippy` with `-D warnings`; the few call sites that
can't reasonably shrink (`#[tauri::command]`s with many parameters, one intentionally
fire-and-forget `spawn_blocking` unlock) carry a targeted `#[allow(clippy::...)]` with a comment,
matching the pre-existing pattern in `cache.rs`. Neither linter catches this project's actual
biggest regression risk — the concurrency/ordering invariants documented in the Concurrency and
Restic Integration sections above (`RepoLocks` ordering, `busy` flags, cancel-path races); those
remain the job of tests and review, not static analysis.

## Running the App

```bash
npm install
npm run tauri dev   # requires Rust installed
npm run tauri build # distributable
npm run clean       # remove dist/ and src-tauri/target/
```
