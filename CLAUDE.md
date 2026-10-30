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
| Shell plugin | `tauri-plugin-shell` — only `open` is exposed (`shell:allow-open`, used for hardcoded http(s) links in App.tsx/SettingsPage.tsx); `"open": true` applies the plugin's default validator (http(s)/mailto/tel only) |
| Memory safety | `zeroize` crate — `MasterKey`/`FullRepository` zeroize sensitive bytes on drop/replace; see docs/data.md |
| Notifications | `tauri-plugin-notification` — shown on backup start/success (changed or unchanged)/warnings/failure, each gated by a global setting (`commands/notify.rs`) |
| Window state | `tauri-plugin-window-state` — `SIZE\|POSITION\|MAXIMIZED` only (never `VISIBLE`), restored by hand in `setup()` then `fit_to_monitor`; min window 1024×640, default 1366×800; see docs/decisions.md |
| Single-instance | `tauri-plugin-single-instance` — prevents multiple processes; focuses existing window on relaunch (a relaunch carrying `--from-autostart` is ignored on purpose — `lib.rs`) |
| Launch at login | `tauri-plugin-autostart`, OS-native entry, gated on the tray setting, carries a `--from-autostart` arg used to gate a hidden launch when auto-unlock is also on; see docs/decisions.md |
| Auto-unlock | `keyring` 4 (`v1` feature), opt-in, stores the *derived* master key; macOS/Windows guaranteed, Linux (Secret Service) best-effort; see docs/data.md |
| ID generation | `crypto.randomUUID()` (native browser API) |
| Tooltip positioning | `@floating-ui/react` — `Tooltip.tsx`, content hovers only (icon-button labels stay on native `title`); see docs/decisions.md for why this one component gets a dependency `ContextMenu.tsx` doesn't need |
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
    ActionButton.tsx      # Canonical row-action button: icon + label, label reveals as text at the `wide` breakpoint — see docs/frontend.md
    AppendOnlyField.tsx   # "Append-only server" checkbox + info tooltip, for `rest:http(s)` paths only — see docs/frontend.md
    BackupWarningsList.tsx # Shared detail view for a backup that completed with warnings (restic exit 3): missing sources first, unreadable files, retention-skipped note, macOS FDA hint — see docs/frontend.md
    Button.tsx            # primary/secondary/danger/ghost variants
    ContextMenu.tsx       # Portal-rendered right-click menu; auto-nudges onto screen; keyboard-operable (focus, arrows/Home/End, Enter/Space, Escape/Tab) — see docs/frontend.md
    EmptyState.tsx        # Empty list placeholder
    icons.tsx             # Canonical glyph icons — single source for every icon used in more than one place; same meaning = same drawing
    ImportExportCard.tsx  # Settings card: export/import bundle + Backrest config.json — see docs/data.md
    InsecureTlsField.tsx  # "Skip TLS certificate verification (--insecure-tls)" checkbox + tooltip, for `rest:https` paths only — see docs/frontend.md
    Input.tsx             # Labeled input with error state; optional inline clear
    Modal.tsx             # Overlay modal dialog
    plan/                 # The plan editor's pieces: PlanHeader (editable title, repo; no buttons), PlanTabs (accessible Files | Settings tablist), PlanFooter (Delete/Cancel/Save + the save error, always in view), FilesTab (the file tree, ListFilesCard, FdaNotice, ExcludeCard, MarkersCard; SourcesCard = plain path list, fallback only), SettingsTab (one scrolling pane: RetentionSection, TagsSection, BandwidthCard, ResticOptionsCard, WebhooksCard) with SettingsNav (the jump list) — see docs/frontend.md
    SegmentedToggle.tsx   # Bordered two/three-way switch used inside cards
    PlanTreeEditDialog.tsx # The tree's two dialogs: "Exclude by pattern…" (scopes + live match count; reached from the row menu) and the add-source confirmation (sources swallowed, rules added or removed)
    PlanTreeExportDialog.tsx # "Use with restic CLI": the exclude file(s) and restic command for the plan
    PlanTreeParts.tsx     # The tree's checkbox-style state marker (incl. the distinct "holds a source" look), Source pill and state labels
    PlanTreePreview.tsx   # The plan editor's file tree: where sources are picked and a preview of what's included, with the reason per row; left-click excludes/includes exactly the item (no dialog), right-click / ContextMenu key / Shift+F10 opens the shared menu — see docs/backup-plan-tree.md
    ProgressBar.tsx       # Determinate/indeterminate progress bar, shared across modals
    RestServerLogin.tsx   # "Server username" / "Server password" pair shown instead of the generic credential rows for `rest:` URLs — see docs/frontend.md
    RetentionFields.tsx   # The five `keep_*` inputs, shared by the plan editor and Settings' default retention card
    Spinner.tsx           # Shared animated loading spinner
    Tooltip.tsx            # Portal-rendered hover tooltip (@floating-ui/react); ContextMenu's styled sibling — for content hovers only, not icon-button labels (those stay on native title) — see docs/frontend.md and docs/decisions.md
    TruncatedText.tsx     # Text with a native `title` shown only when actually cut off; callers supply the truncation classes (bare `truncate` for free-text cells, `max-xl:truncate` for compact-only ones) — see docs/frontend.md
    Sidebar.tsx           # Icon rail below 1280px (`max-xl:`); left nav with app icon; "Lock" item at the bottom of the nav list (shared handleLock with tray/menu); a footer status strip below the nav list (not a nav item) toggles the Activity panel, showing "N tasks running"/"No background activity" from activeTaskCount()
    ActivityPanel.tsx     # Right-side overlay drawer surfacing background activity (indexing, scheduler backups, stats, mirrors); opened via the Sidebar's status strip, open state owned by App.tsx — see docs/concurrency.md
  lib/
    types.ts              # Shared TS types (Repository, Snapshot, BackupPlan, etc.); isRemoteRepo() helper
    backends.ts           # detectBackend + credential hints for S3/B2/REST — see docs/restic.md
    invoke.ts             # Typed wrappers over tauri invoke()
    activity.tsx          # ActivityProvider: indexing, scheduler backups, stats, mirrors via the task event bus — see docs/concurrency.md
    format.ts             # formatBytes, formatSize, formatDate, formatDuration, dbSizeHint (Settings DB-size hint), etc.
    config.ts             # MIN_RESTIC_MAJOR/MINOR constants for version warning
    utils.ts              # needsFullDiskAccess(path) — macOS protected-path check for one path (callers iterate, e.g. via planReadPaths)
    theme.tsx             # ThemeProvider + useTheme(); persists to localStorage
    cron.ts               # parseCronToSimple/buildCronExpr — pure cron helpers, unit-tested
    difftree.ts           # computeChildren/toSegments — pure tree-building for DiffPage, unit-tested
    menuNav.ts            # `nextEnabled`: pure keyboard navigation for ContextMenu, unit-tested
    scrollSpy.ts          # `activeSectionIndex`: which Settings section the plan editor's jump list highlights for a scroll position, unit-tested
    breakpoints.test.ts   # Pins Sidebar's `useIsCompact` 1280 query to Tailwind's stock `xl` (and fails if tailwind.config.js overrides it)
    node-stubs.d.ts       # Ambient node:* types for the file-reading tests (the project has no @types/node)
    planTreeEdit.ts       # Pattern-scope options, glob escaping, the edit a menu action asks for (`editActionFor`, `needsConfirmation`, `hasExactRule`) and the Undo-line summary — pure, unit-tested
    planForm.ts           # The plan editor's form as pure functions: state, load a plan, validate (naming the tab or Settings section an error belongs to), build the plan (exclude comments kept), URL location (`planLocation`, old-link compat), per-section "non-default" markers — unit-tested
    planTree.ts           # Row states, reason text, row flattening, and the left-click / right-click-menu decisions (`leftClick`, `menuEntries`) for PlanTreePreview — pure, unit-tested
  pages/
    AuthPage.tsx          # Master password setup (first launch) and unlock screen
    RepositoriesPage.tsx  # Add/open/duplicate/delete/edit repos, remote URL + credentials, read-only flag, stats refresh, mirror, prune, check, Index All — see docs/frontend.md
    SnapshotsPage.tsx     # Snapshot table; stale-while-revalidate cache; restore, copy, tag, delete, per-snapshot index; wide-only Size column (cached summary size) — see docs/frontend.md
    BrowsePage.tsx        # File tree inside a snapshot; restore, tag management, search entry point — see docs/frontend.md
    SearchPage.tsx        # Full-text file search within one snapshot; index state machine — see docs/frontend.md
    RepoSearchPage.tsx    # Full-text file search across every indexed snapshot in a repo; Index All batch — see docs/frontend.md
    DiffPage.tsx          # Diff viewer between two snapshots; client-side tree, restore from diff
    BackupPlansPage.tsx   # List/run/delete plans; per-plan "Last run" line (newest `backup_history` attempt, manual or scheduled); backup modal with progress; auto-applies retention — see docs/frontend.md
    BackupPlanEditPage.tsx # Create/edit plan shell: loads/saves the form (lib/planForm.ts); header (editable name h1, repository), Files | Settings tabs (Settings = one scrolling pane with a jump list; `?tab=…&section=…` in the URL) and a persistent Delete/Cancel/Save footer (components/plan/) — see docs/frontend.md
    SchedulesPage.tsx     # List schedules; toggle/delete; read-only-repo warnings
    ScheduleEditPage.tsx  # Create/edit schedule (cron expr, backup plans); read-only-repo badges
    LogsPage.tsx          # Backup history log; paginated; expandable error rows
    SettingsPage.tsx      # Theme, tray, launch-at-login, auto-unlock, restic path, default retention, prune all, import/export, cache — see docs/frontend.md and docs/decisions.md
src-tauri/
  src/
    main.rs               # Calls restic_gui_lib::run()
    lib.rs                # Tauri builder; registers commands; manages app state; native menu bar; tray — see docs/concurrency.md
    tasks.rs              # Unified `task` event bus (TaskEvent, OperationCtx) — see docs/concurrency.md
    gpu_compat.rs         # NVIDIA+Wayland env workaround (gated) + AppImage fixups/`is_appimage` — see docs/decisions.md
    cache_warmer.rs       # Background sweep: snapshot refresh + auto-indexing + the ~5-minute orphan-cleanup tick — see docs/concurrency.md
    scheduler.rs          # Background tick runs due schedules via execute_backup — see docs/concurrency.md and docs/decisions.md
    commands/
      mod.rs                # get_restic_path(); NoConsole trait for Finder-launched PATH
      auth.rs               # Setup/unlock/lock master password; auto-unlock — see docs/data.md
      crypto.rs             # Argon2id key derivation, AES-GCM encrypt/decrypt
      keychain.rs           # Auto-unlock's OS credential-manager access — see docs/data.md
      portal.rs             # AppImage-only file-chooser portal probe (`file_portal_missing`); false everywhere else — see docs/decisions.md
      plan_rules.rs         # Verified port of restic's exclude matching: decides what a plan includes — see docs/backup-plan-tree.md
      plan_edit.rs          # Turns tree clicks into verified source/pattern edits (exact-item exclude/include, carve-out chains, glob escaping, `# Tree:` comments, collateral check) and renders a plan as restic CLI files — see docs/backup-plan-tree.md
      plan_edit/lines.rs    # Rule-list bookkeeping for tree edits: a path's exact lines, `# Tree:` comments, carve-out scaffolding (`orphan_scaffolding`) — see docs/backup-plan-tree.md
      plan_edit/scan.rs     # Bounded walks for tree edits: `find_collateral` (refuse an edit that flips anything but the clicked item) and `count_brought_back` — see docs/backup-plan-tree.md
      plan_listfiles.rs     # Reads `--files-from` list files like restic (trim, comments, Go filepath.Glob) into read-only extra sources for the tree — see docs/backup-plan-tree.md
      plan_tree.rs          # Commands behind the plan editor's tree preview: list folders, classify paths, streamed subtree scans — see docs/backup-plan-tree.md
      notify.rs             # Global notification toggles (started/success-changed/success-unchanged/warnings/failures); classify_success is pure/unit-tested — see docs/restic.md
      backends.rs           # Backend credential registry (S3/B2/REST) — see docs/restic.md
      repo.rs               # Repo CRUD, stats, restic path/version, prune, FDA checks — see docs/restic.md and docs/concurrency.md
      repo_locks.rs         # RepoLocks: per-repo shared/exclusive lock registry — see docs/concurrency.md
      rest_probe.rs         # `detect_append_only`: the `DELETE /data/<64 zeros>` probe that suggests a rest-server's `--append-only` mode; pure helpers unit-tested — see docs/restic.md
      snapshot.rs           # List/delete/tag snapshots; execute_backup; copy/mirror; retention — see docs/restic.md and docs/concurrency.md
      browse.rs             # File listing, restore, indexing (single + batch) — see docs/concurrency.md and docs/decisions.md
      backup_plan.rs        # List/save/remove backup plans; `validate_plan_sources` (paths-or-list-files, list-file plans need a tag) — see docs/restic.md
      schedule.rs           # List/save/remove/toggle schedules; cron helpers reused by scheduler.rs
      transfer.rs           # Export/import bundle + Backrest config.json import — see docs/data.md
      webhook.rs            # Per-plan webhook delivery (generic/Discord/Slack/Teams presets + Custom {placeholder} templates); test_webhook; preview_webhook renders build_body for the edit page; pure build_body/build_message/interpolate unit-tested — see docs/backend.md
      cache.rs              # AppDb (SQLite state), MasterKey, operation handles — see docs/concurrency.md
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
| What a backup plan includes/excludes (rule engine, file tree, tree edits) | `docs/backup-plan-tree.md` |
| Before "fixing" something that looks like a bug or inefficiency | `docs/decisions.md` (check the index below first) |

## Restic Integration

Full detail (env-var policy, streaming vs. one-shot calls, `mirror_repo`'s queueing, backend
credentials incl. REST, stats-cache policy): **docs/restic.md**.

Retain always: restic's binary path is user-configurable (defaults to `restic` on `$PATH`); every
command sets `RESTIC_REPOSITORY`/`RESTIC_PASSWORD` as **env vars, never process args** (a repo with an
empty stored password gets `--insecure-no-password` instead and no `RESTIC_PASSWORD`); every
restic `Command` must set `.stdin(Stdio::null())` (Windows Scoop-shim spawn failures otherwise)
alongside `.no_console()`; an `async fn` `#[tauri::command]` must **never** call
`std::process::Command` inline — always `spawn_blocking` (streaming) or `run_restic_blocking`
(one-shot) — or it blocks a shared tokio worker and starves every other async command plus the
`AppDb` lock. A write op or backup that fails on restic's "already locked" runs plain `restic unlock` **once**
(stale locks only — never `--remove-all`) and retries, bounded via `repo::lock_retry_step`; see
docs/restic.md "Stale locks" and docs/decisions.md. A repo can be marked **read-only** (`--no-lock` on every read op via
`apply_repo_flags`); every write op instead calls `ensure_writable` and refuses outright. A stats
refresh (`fetch_and_cache_stats`) runs restic **twice** — default mode for the restore-size figures,
then `--mode raw-data` for the on-disk stored size (`ResticStats.raw_size`) — with the second call's
failure deliberately non-fatal to the refresh; see docs/restic.md. Retention runs `forget --prune` unless the repo has `defer_prune` set (per repo, since cloud early-deletion fees depend on where it's stored); explicit prunes still prune it — see docs/restic.md "Deferred prune".

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

Full detail (eight stateful frontend consumers, why `operationId` not `repoId`, the mirror/index-batch
per-run registries): **docs/concurrency.md**.

Retain always: `tasks.rs` defines a uniform `task` event (`TaskEvent`) layered **on top of**,
never instead of, existing detailed feeds (`backup:progress`, etc.). Envelope: `operationId`
(unique per operation instance — the correlation key, since `repoId` alone can't distinguish
concurrent operations), `kind`, `phase` (`pending`|`started`|`progress`|`cancelling`|`cancelled`|
`finished`|`failed`), `repoId`, `targetId`, `origin`, `progress`, `error`, `at` (plus `retentionSkipped`, set only on a finished `backup`). Every
restic-shelling operation is wired via `OperationCtx`, except pure-DB reads (`list_snapshots`,
`get_repo_stats`), the continuous 60s `cache_warmer` snapshot-refresh tick, and the still-unwired
`refresh_snapshots` and `init_repo` (see docs/concurrency.md). `TaskKind::Cleanup` (`clean_cache`, the
"Clean Orphaned Data" button, and the automatic `maybe_run_cleanup`) never shells out to restic and
isn't scoped to a single repo, so it emits `repoId: ""`; Prune All and `testConnection` do too, so a
consumer keying off `repoId` must not assume every event carries a real one.

## Security Architecture

Full detail (rotation transaction, auto-unlock keychain design and trade-offs): **docs/data.md**.

Retain always: master password → Argon2id → 32-byte key; AES-GCM encrypts a verification
plaintext; **the password itself is never stored**. `MasterKey` is `Mutex<Option<[u8; 32]>>` —
`None` when locked, every restic command fails with "App is locked". Repo passwords and backend
credentials are AES-GCM-encrypted under the master key. Auto-unlock (opt-in, default off) stores
the *derived* key, never the password, in the OS credential manager — guaranteed on macOS/Windows,
best-effort on Linux via the D-Bus Secret Service (see docs/decisions.md).

## Persistence & Caching

Full detail (schema, migrations, stale-while-revalidate patterns, cache warmer): **docs/data.md**.

Retain always: single SQLite `app_data.db` via `AppDb`, one shared `Mutex<Connection>` — a slow
synchronous query on a core async-runtime thread starves every other `AppDb`-touching command.
Any new command doing DB work slow enough to notice should be `async fn` + `spawn_blocking`.
Schema migrations use `add_column_ignoring_duplicates` (only "duplicate column name" is
tolerated; any other failure aborts startup) — never a bare `let _ = conn.execute_batch("ALTER …")`.

## Settled decisions — do not re-flag without reading `docs/decisions.md`

These were deliberately kept as-is after prior audits raised them. Read the linked entry before
proposing a change — several are pinned by a named test or reference a confirmed regression.

- A restic backup that exits **3** (snapshot saved, some source data unreadable) is **completed
  with warnings**, not a failure (issue #36): `execute_backup` classifies the exit code through the
  pure `classify_backup_exit` (3 counts only when the summary carried a `snapshot_id`, otherwise a
  failure), logs it with its `snapshot_id` plus `warning_count`/`missing_source_count`/
  `warnings_json`, appends the snapshot to the cache, and notifies under the `Warnings` category.
  A plan source path that no longer exists is a stronger warning (its own wording), found by the
  app's own post-run `stat` (`find_missing_sources`), not by matching restic's error text.
  **Retention is skipped only when a source path is missing** — never merely because files were
  unreadable, or a chronically-warning plan (e.g. `/etc` as non-root) would never prune. The backend
  decides this once (`BackupOutcome::retention_skipped`, also carried on the finished `backup` task
  event as `retentionSkipped`); the scheduler, BackupPlansPage and the Activity panel all read the
  flag rather than re-deriving it. Don't re-add a notification-only "warnings" path or make warning
  runs apply/skip retention differently — see docs/decisions.md
- Webhook delivery is fire-and-forget **by design, with silent failures**: all of a plan's
  endpoints are POSTed sequentially on one detached `spawn_blocking` thread, every result
  discarded (`let _ =`), no retry/log/notification — a dead webhook must never fail or delay
  the backup, and one failing endpoint never stops the ones after it (a failure can only
  delay them, bounded by the 10s per-request timeout). Sequential, not parallel, is the
  accepted trade-off for the typical 1–3 endpoints per plan; `test_webhook` (Send Test) is
  the only path that surfaces delivery errors. Don't add fire-time retry/logging without
  treating it as a deliberate design change — see webhook.rs in docs/backend.md
- Lock failures self-heal with a **single** plain `restic unlock` + bounded retry (issue #43) — not a
  periodic sweep (rejected: background cloud traffic, fixes nothing until the next tick), never
  `--remove-all`, and not for read ops or copy/mirror; every path goes through `lock_retry_step`. See
  docs/decisions.md before widening it
- `commands/plan_rules.rs` is a literal port of restic's exclude-pattern matching (Go's
  `filepath.Match`, `filter.Match/ChildMatch/List`, `RejectIfPresent`), including quirks such as the
  `**` expansion bound; it is verified against Go's/restic's own test tables and a differential test
  against the real restic. Don't "simplify" or "fix" it without re-running that test — see
  docs/backup-plan-tree.md. Platform path rules live in `PathStyle` (Unix | Windows; UNC paths
  unsupported); `.github/workflows/restic-compat.yml` runs the restic-backed `#[ignore]` tests on
  Linux/macOS/Windows against restic 0.17.3 and 0.19.1. The two differ on **explicit source paths**
  (restic#5767: exempt from excludes/markers from 0.19, judged like any node before) — the engine
  models both (`PlanRules::judge_sources`, set from `restic version` via `TreeEnv`), and the
  harnesses compare each restic against its own rule, so don't hard-code one
- Plan-tree clicks (`plan_edit.rs`) act on the **exact item** and never touch a broad rule: include
  deletes P's own exact line(s), else appends the specific `!P`; exclude deletes lone `!P` lines (plus
  orphaned carve-out scaffolding — only if every folder it reopened ends up excluded again; that gate and
  `orphan_scaffolding`'s per-pair rule stay together), else appends P's exact rule. A `!` on a folder also
  brings back what other rules hid inside it (restic F1): accepted, disclosed in the Undo line, not worked
  around. There is deliberately **no** "include the whole excluded folder" option; the collateral check
  refuses an edit that flips anything but P, its subtree and the path to P (pattern-kind and source edits
  exempt); `# Tree:` comments are deleted only with their own rules; click-twice identity holds for lines
  the tree wrote (a deleted hand-written line returns only as an equivalent included set). Re-run
  `cargo test restic_confirms -- --ignored` before relaxing any of it — see docs/backup-plan-tree.md and
  docs/decisions.md
- Backend credentials use their own nonce/ciphertext columns, not folded into the password blob
- `apply_backend_env` filters reserved credential keys itself, not just `validate_credentials`
- `validate_credentials` deliberately allows arbitrary keys for `BackendKind::Rest`
- `--insecure-tls` is a per-repo, user-acknowledged flag for `rest:https://` repos (`apply_tls_flags` at
  every restic `Command` site, since restic has no env var for it); `check_tls_compat` refuses a
  copy/mirror pair where it would disable verification on an unflagged TLS-verifying repo; the
  recommended fix is trusting the CA in the OS store; client certs/`--cacert` are deferred — see
  docs/decisions.md
- `rest:` failure hints (`backends::rest_error_hint`) are appended after restic's text and advisory
  only; `test_repo_connection` fails fast on a `rest:` repo's first restic retry line (and has a 45s deadline) via `run_restic_with_timeout` because
  restic retries unreachable backends for minutes — see docs/decisions.md
- `rest-server --append-only` repos carry a stored `append_only` flag (set by the user or suggested by
  `rest_probe`'s `DELETE /data/<64 zeros>` after a successful connection test): `ensure_can_delete`
  blocks delete/tag/retention/prune (`tag` too — restic's tag edit leaves a duplicate snapshot when the
  delete 403s), backup/unlock/copy-into stay allowed, and retention is folded into
  `BackupOutcome::retention_skipped` rather than failing; the probe only ever ticks the box (never
  clears it) and is skipped for read-only repos — see docs/decisions.md
- `"rest:"` stays listed in `REMOTE_PREFIXES` even though a dedicated `detect_kind` arm exists
- Sync `#[tauri::command]`s are not wrapped in `spawn_blocking` (Tauri already offloads them)
- `scheduler.rs` calls sync `apply_retention` directly, not via `spawn_blocking`
- `list_snapshots`, `get_snapshot_index_status`, `get_repo_stats` don't emit on the `task` bus
- `get_repo_stats` is cache-only and must never fall through to a live `restic stats` call
- `browse_cache_files.parent_path` duplicates a prefix of `path` on purpose (index speed)
- `remove_repo` deliberately does **not** cascade into `browse_cache_files`/`indexed_snapshots`
  (keyed by `snapshot_id`, not `repo_id`); that delete was unbounded and made repo removal hang
  for minutes. `clean_cache` sweeps the resulting orphans in bounded batches — see docs/data.md.
  Retention (`apply_retention`) is the more common orphan source in practice, not repo removal.
- `clean_cache` is `mark_orphans` (instant — stamps `indexed_snapshots.orphaned_at`, drops
  `browse_cache_status`) followed by `drain_orphans` looped in `CLEAN_CACHE_BATCH_ROWS`-row
  batches, each its own transaction, yielding the `AppDb` connection mutex between batches — the
  same one-transaction-holds-everything problem `remove_repo`'s old cascade had, fixed by
  batching the delete instead of shrinking it. Marking before any file-row delete means an
  interrupted run (Stop, crash, quit) never leaves a snapshot claiming to be indexed with its
  file rows partially gone — see cache.rs's doc comments on both functions.
- Orphan cleanup runs two ways: the "Clean Orphaned Data" button (`clean_cache`, `origin:
  Manual`) and, since `evict_snapshots`'s removal made it safe, an automatic tick in
  `cache_warmer.rs` every 5th 60s cycle (`maybe_run_cleanup`, `origin: Background`). Both call
  the same `run_cleanup` (`commands/cache.rs`); `CleanupHandle.busy` serializes them against
  each other. An earlier ticker-driven attempt was built and rolled back before this one — see
  docs/decisions.md for why that round was unsafe and what changed since. `maybe_run_cleanup`
  skips its tick (never queues) while the app is locked, or when `AppDb::has_cleanup_work()` — a
  cheap read-only `EXISTS` probe, kept in sync with `mark_orphans` by
  `has_cleanup_work_agrees_with_mark_and_drain` — finds nothing to do; there is no Settings
  toggle, since cleanup shells out to nothing and only deletes rows nothing references. Don't
  gate this on per-repo cache freshness — a repo that hasn't refreshed yet (remote with
  `remote_auto_refresh` off, unreachable, or just locked) keeps its last-written
  `snapshots_cache` rows rather than losing them, so the failure mode is under-cleanup, which
  the next sweep (automatic or manual) self-heals; see docs/decisions.md. `maybe_run_cleanup`
  spawns its `run_cleanup` call detached (like `trigger_sweep` above it) rather than awaiting it
  inline, so a large backlog draining over several batches never stalls that tick's own
  `refresh_all_snapshots`/index sweep — `busy`'s `compare_exchange` inside `run_cleanup` is what
  keeps two runs from overlapping, not the call site, so detaching doesn't change serialization.
  `intern_snapshot` (used by every indexing write) upserts and clears `orphaned_at` on conflict
  instead of `INSERT OR IGNORE`ing, so a snapshot that resurfaces in `snapshots_cache` and gets
  re-indexed while a `drain_orphans` batch loop for it is still running is un-marked before any
  file rows are written — without this, indexing would write into a row a concurrent drain is
  still deleting from, leaving it `browse_cache_status = complete` with rows silently missing and
  nothing left to retry it. Don't revert that upsert back to `INSERT OR IGNORE` — see docs/decisions.md.
  The wider index-run × cleanup interleaving family is closed the same preventive way: chunk
  writes verify their mapping on `(id, snapshot_id)` (SQLite recycles freed rowids — `id` alone
  could match a different snapshot's recycled mapping), `evict` keeps rows still shared with
  another repo **that has indexed the snapshot** (the guard keys on `browse_cache_status` rows —
  the consumers — not `snapshots_cache` listings; a repo that forgot its copy keeps its
  'complete' status, a repo that only lists the snapshot keeps nothing) and is one transaction,
  an index run's terminal status writes are gated two ways: the success-path 'complete' write
  goes through `set_browse_status_complete_if_live` (row present **and** `indexed_snapshots`
  still maps to the exact `snap` id this run wrote against — needed because a *different* repo's
  Clear Index can drop the shared mapping without touching this repo's own status row), the
  failure-path 'pending' write through `set_browse_status_if_present` (row present is enough — a
  plain UPDATE, never resurrects); either 0 rows affected means evicted mid-run, which aborts the
  run, evicts its own re-written rows, and returns an Err so the warmer's sweep doesn't
  immediately re-pick and re-complete the snapshot the user just cleared. Its 'in_progress' entry
  write goes through `set_browse_status_if_listed` (never creates a status row for an unlisted
  snapshot), and all three runner sites share one `run_index_and_report` — see
  docs/decisions.md.
- `AppDb::evict_snapshots` (wipe a repo's entire `snapshots_cache` with nothing to replace it)
  was removed outright, not just left unused — it's what created the wipe race above. `set_snapshots`
  is diff-based (deletes only confirmed-dropped ids, upserts every id in the fresh listing) rather
  than delete-all-then-insert-all, so it can never manufacture an artificially-empty cache; every
  caller that used to reach for a wipe (`delete_snapshot`, `copy_snapshot`, `mirror_repo`,
  `execute_backup`/`apply_retention`'s re-fetch-failure fallback) now either fetches real data
  first or does nothing on failure. Don't reintroduce a caller that clears `snapshots_cache` with
  no real listing in hand — see docs/decisions.md.
- `clear_browse_cache` ("Clear All Cache", which calls `AppDb::clear_cache`) deliberately does **not** `VACUUM` — its `DELETE FROM table;`
  calls have no `WHERE` clause, so SQLite's truncate optimization already makes them cheap
  regardless of row count; `VACUUM` scales with total DB size instead and would hold the same
  `AppDb` connection mutex for however long that takes, the same freeze class `clean_cache`'s old
  unbounded transaction caused. Reclaiming space is `compress_database` ("Compress Database")'s job
  alone — see docs/decisions.md. Compaction is never automatic (issue #44, a 100 GB `app_data.db`):
  `checkpoint_and_size` returns a `DbSize` (total + freelist-based `reclaimable_bytes`) and Settings
  shows it with a compress-or-"that's live index data" hint (`dbSizeHint`, format.ts) instead. Path
  deduplication and `auto_vacuum = INCREMENTAL` were analyzed and deliberately parked — don't build
  either without the evidence named in docs/decisions.md
- Stored repo size (`ResticStats.raw_size`) comes from `restic stats --mode raw-data`, not a `du`/filesystem walk (works for remote repos too); a failed raw-data call is non-fatal to the refresh
- File search uses `LIKE '%query%'` (leading wildcard, no index) — accepted, not an oversight
- `cached_at` columns are written but not read yet — kept for a future TTL feature
- `panic = "abort"` is deliberately not set in the release profile
- Known, deferred frontend duplication (search/index pattern, FileIcon, index-batch UI, etc.)
- Displayed times use the web view's `Intl`/ICU zone rules, not the OS's: a stale bundled ICU (the
  AppImage's, e.g. Morocco in issue #40) shows every time off by the rule difference. Cosmetic and
  affects very few users — a declined fix, not an oversight; see docs/decisions.md before adding
  any offset/zone logic
- Linux/Wayland title-bar buttons freezing after a tray restore was an upstream `tao` 0.35 bug
  (its custom Wayland header's `EventBox` sat above the buttons), fixed by tao ≥ 0.36 / tauri ≥
  2.12 (enforced by the `tauri = "2.12"` manifest floor). Don't pin tauri below 2.12 and don't add an app-side workaround without re-verifying on
  Wayland; see docs/decisions.md
- Backup progress bars are non-monotonic by design (restic's own `percent_done` behavior)
- `IndexHandle::gate` must stay one app-wide mutex — never split per-batch or per-repo
- `gpu_compat::apply()`'s NVIDIA+Wayland workaround is gated, not applied unconditionally
- Linux file dialogs use `tauri-plugin-dialog`'s `xdg-portal` feature, not the default `gtk3` one — `rfd` silently reverts to GTK if `gtk3` is enabled anywhere in the dependency graph, so don't add it back
- `bundle.targets` in `tauri.conf.json` is an explicit list (`deb`/`appimage`/`rpm`/`dmg`/`nsis`/`msi`).
  AppImage was dropped in v0.0.8 for Mesa 25+ black windows and re-added (issue #28) once
  tauri#15665/#15781 were verified fixed in tauri-bundler 2.10.0 / `@tauri-apps/cli` ≥ 2.12.0 —
  that CLI version is a floor (`"@tauri-apps/cli": "^2.12.0"` in package.json), don't pin lower. `"deb"` must stay listed (the AppImage bundler
  builds from the deb output tree). App-side, AppImage-only: `gpu_compat::apply_appimage_fixups`
  unsets the dangling `GST_PLUGIN_SYSTEM_PATH_1_0`, and `commands/portal.rs` + an `App.tsx` banner
  warn when the file-chooser portal is missing (an AppImage has no `depends`). A
  `WEBKIT_DISABLE_DMABUF_RENDERER` AppImage gate was deliberately not added. Read docs/decisions.md
  ("AppImage — re-enabled") before touching any of it, and re-test on a Mesa 25+ host before a release
- Windows and Linux install no native menu bar — the menu is built and `MenuState` managed on
  every platform, but `setup()` drops it on Windows/Linux; only macOS installs it (replaces the
  old Windows-only "fold the app submenu into File" workaround, whose cfg machinery was deleted).
  Locking's tray-independent path is the Lock item at the bottom of the sidebar's nav list
  (`Sidebar`'s `onLock`,
  shared `handleLock` in `App.tsx`); don't reintroduce a Windows menu bar, the File-fold, or hide
  macOS's system menu bar — see docs/decisions.md
- `--exclude-cloud-files`/`--use-fs-snapshot` are emitted only on Windows (`build_backup_args`), even
  though restic 0.19 accepts the former on macOS — gated on what the 0.17 minimum supports, since
  distro restic lags (Ubuntu ships 0.18.x) and an unknown flag fails the whole backup; see
  docs/decisions.md before widening it
- Plans can source from `--files-from` list files and exclude via `--exclude-file`/`--iexclude-file`
  files (no version/platform gate — all predate restic 0.17). **Sources and excludes are both
  combinable in the editor**: folders picked in the tree and path-list files save together (the old
  Paths | List files either/or tabs saved only the active side; they went when the tree became the
  source editor), as do exclude patterns and exclude files. A plan **with any list file must have a tag**,
  retention or not (editor error + `validate_plan_sources`): its snapshots' paths live in the list
  files, so a tag is the only handle retention has, and requiring it from the first save means adding
  retention later still covers the plan's whole history. `apply_retention`'s `check_retention_scope`
  additionally refuses a `forget` with no tags *and* no paths (it would prune the whole repo) — the
  backstop for imports, which skip the save-time check on purpose. Whitespace-only tags never count
  as tags and never reach restic (`non_blank_tags`, used by the backup/retention arg builders, the
  scope check and `validate_plan_sources`): restic's `--tag ""` matches *untagged* snapshots. A hidden auto-tag (plan-name or
  plan-id based) was considered and rejected: it would show on every such snapshot in the UI and CLI,
  and names aren't unique — don't add one without re-reading docs/restic.md
- Retention `keep_*` of `0` means **unset**, like `limit_upload`/`pack_size` — not "keep none".
  (Verified against restic 0.19: `--keep-last 0` alone exits "no policy was specified, no
  snapshots will be removed", so it was never a deletion risk — it made every retention run fail.)
  `build_retention_args` drops zeros; every call site gates on `has_effective_retention`
  (`snapshot.rs`) rather than `is_some()`; `apply_retention` fails cleanly with no effective rule;
  the editor rejects 0/negative/decimal counts (`parseRetention`, shared with Settings' default); and `activity.tsx`/`BackupPlansPage.tsx` mirror the same
  `> 0` test. Keep those in agreement — the Activity row waits on a `forget` op only when
  retention will actually run. An imported plan with a hand-set zero is simply inert; a decimal,
  string or negative count in an import can't occur — `RetentionPolicy` is `Option<u32>`, so the
  bundle/Backrest parse fails (whole file refused), pinned by tests in `transfer.rs`.
- The Settings **default retention policy** (issue #48) is a **creation-time template**, off and
  empty by default: the plan editor copies it into a *new* plan's fields and nothing reads it at
  backup/`forget` time. Don't turn it into run-time inheritance — retention deletes snapshots, so a
  Settings edit must never re-prune existing plans (and inheritance would need an "inherit" vs
  "none" plan state through `apply_retention`, the scheduler and the activity reducers). Its load
  (`load_default_retention`) fails *closed* — an error prefills nothing, unlike `notify::load`. See
  docs/decisions.md
- **Deferred prune** (issue #49): retention is `forget --prune` unless the repo has `defer_prune` set
  (edit-modal checkbox "Prune after retention", ticked by default and stored inverted — a positive
  label on purpose, the original "Don't prune…" double negative confused users). It's per **repo**, not
  per plan or global (cloud early-deletion fees depend on where the repo lives), and explicit
  "Prune…"/"Prune All" still prune a deferred repo. A failed flag lookup **fails closed** (no prune);
  a read-only repo keeps its stored choice but the row hides the badge. `last_pruned_at` is
  best-effort bookkeeping. Bulk snapshot delete is one `restic forget id1 id2 … [--prune]` (the
  `delete_snapshot` command takes a list), so a ticked "prune" runs once — see docs/decisions.md
- Controls use the native arrow cursor, not the web-style hand, with **no Settings toggle** (issue
  #35): a base-layer rule in `index.css` (`button, [role="button"] { cursor: default }`) overrides
  Tailwind preflight's `cursor: pointer`, no `cursor-pointer` classes remain, and the sidebar's
  `NavLink`s (real `<a href>`, which webviews hand-cursor regardless of CSS) carry an explicit
  `cursor-default`. `cursor-help` (tooltip triggers) and `cursor-not-allowed` are meaningful and
  stay. Don't reintroduce `cursor-pointer`; give clickable non-button surfaces a hover style
  instead — see docs/frontend.md
- Launch-at-login has no `app_settings` row (OS entry is the sole source of truth)
- Auto-unlock toggle is deliberately not gated on launch-at-login or the tray setting
- Auto-unlock is offered on Linux too (D-Bus Secret Service), but explicitly best-effort — it
  degrades to the password prompt whenever the store is absent or locked, never a silent failure.
  Don't reintroduce the kernel keyutils store (genuinely non-persistent, unlike Secret Service)
  and don't turn `keychain::is_supported()`'s Linux probe into a real D-Bus round trip — see
  docs/decisions.md
- `.app_name("resty-desktop")` on the autostart builder must not be dropped
- A login launch shows a hidden window only when tray + auto-unlock + launch-at-login are all on
  (`--from-autostart` arg, gated via the standalone `should_start_hidden`); any other state,
  including a failed auto-unlock, always shows the window
- `App.tsx`'s `show_main_window()` fallback is gated on `hasBeenUnlockedRef` (first-unlock-only):
  it force-shows the window on any non-unlocked state before the session's first unlock, but
  never after — a mid-session "Lock Now" from the tray must leave a hidden window hidden
- The tray icon is created in `setup()` (locked variant) whenever `tray_enabled` is on, not lazily
  after unlock; `activate_tray` swaps the locked/unlocked variant in place via `set_menu`/
  `set_tooltip` on the existing icon rather than rebuilding it — rebuilding leaks the old OS icon
  (Tauri's `TrayIconBuilder::build` keeps its own resource-table clone), which is exactly the bug
  that made the tray keep showing "Locked" after unlock; the unlocked variant also carries a
  tray-only "Lock Now" item (needed once unlocked-and-hidden becomes a reachable state) whose
  handler deliberately never shows the window
- `handleTrayToggle` clears launch-at-login on *enabling* the tray, not only on disabling
- `set_launch_at_login`'s disable path checks `is_enabled()` first (Windows idempotency)
- `reset_app` also best-effort clears the OS autostart entry, not just DB tables
- The Windows `Run` registry value `auto-launch` writes is unquoted — not fixable from app code
- Window size is persisted by `tauri-plugin-window-state` with **no `VISIBLE` flag** (it would
  `show()` the window and break the hidden login launch), restored manually then shrunk by
  `fit_to_monitor` (the plugin restores physical px unclamped), and `reset_app` deliberately does
  **not** delete the state file (the plugin rewrites it at exit). The sub-1280 compact layout's *structural* changes (icon rail, hidden/stacked columns, header wrap) are
  `max-xl:`/`max-mid:`-additive only — never edit a class that applies at ≥1280 for them, and keep `truncate`/
  `max-w-0` `max-xl:`-prefixed there (full-width truncation would clip
  `overflow-hidden` tables). Deliberate exceptions, all because a long user-chosen name must never wrap,
  overflow or scroll at any width: free-text table cells (Logs Plan/Repository, Snapshots Host/Tags,
  Browse/Diff names) are shrinkable (`max-w-0` + `w-[N%]`) + `truncate`; the Repositories list is
  `grid-cols-1` with truncated rows; page-header titles next to a toolbar truncate; `Button` is
  `whitespace-nowrap`; `Modal` titles wrap with a `flex-shrink-0` close button; and no-wrap/`min-w` guards
  (Snapshots Paths, Browse Size/Modified, Diff Change, small badges/labels) apply at every width — they
  only change a layout that would otherwise wrap. The column *percentages* on Logs/Snapshots do reshape how
  spare width is shared at full size (name columns get it). Tables use
  `overflow-hidden`, never `overflow-x-auto` (the Logs `min-w-[800px]` + `overflow-x-auto` were removed).
  See docs/decisions.md and docs/frontend.md
- `react-router-dom` stays on 6.x — its two audit advisories are unreachable in this app
- Dark-mode `--tw-gray-500` (`139 148 163`) and light-mode `--tw-gray-500` (`88 103 126`) are
  deliberately not stock Tailwind values — both tuned so gray-500 text passes WCAG AA (≥4.5:1)
  on every surface it sits on, enforced by `src/lib/contrast.test.ts`; bare text never goes
  darker than `text-gray-500` (gray-600/700 are border/divider colors only) — see docs/frontend.md

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

Three modes: System (the default when nothing is stored), Dark, Light; `:root` carries the dark values. Stored in `localStorage`; applied as `dark`/`light`/`system` class on `<html>`. All theme-sensitive colors route through CSS custom properties in `src/index.css`, extended in `tailwind.config.js` (`gray.50–950`, `blue/green/red.300/400/700/900`, `amber.300/400/500/700/900`, `purple.400`, `yellow.400`).

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
- Colors outside the extended set (`blue-500/600`, `red-500/6/8`, `yellow-*` other than `yellow-400`, `purple-*` other than `purple-400`) are NOT theme-mapped (`yellow-400`/`purple-400` *are* mapped — don't "fix" their uses) — intentional for colored-background elements like primary/danger buttons where white text is always on a dark surface, where the surface itself (not the page background) sets the contrast context.
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
deliberately not included. `RESTY_DISABLE_GPU_WORKAROUND` opts out without a rebuild. The same
module also unsets the dangling `GST_PLUGIN_SYSTEM_PATH_1_0` inside an AppImage
(`apply_appimage_fixups`, independent of that gate and opt-out) — see the AppImage entry in
docs/decisions.md.

## Releases

`.github/workflows/release.yml` — triggered by `v*` tag; a `test` job (frontend + Rust tests) gates the build; builds on ubuntu-22.04, ubuntu-22.04-arm, macos-latest, windows-latest via `tauri-apps/tauri-action@v0`; creates a draft GitHub Release. Annotated tag message becomes release body. The `build` job carries `permissions: contents: write` and the `github.server_url` check that skips it on non-GitHub CI; the `test` job always runs.

Pre-built macOS binaries are not notarized: `sudo xattr -rd com.apple.quarantine /Applications/Resty\ Desktop.app`.

To cut a release, use `/tag` then:
```bash
git push origin main
git push origin v0.0.X
```

## Testing

- Frontend tests use **Vitest**; test files live alongside source as `src/lib/*.test.ts`.
- Rust unit tests use `#[cfg(test)]` modules in `lib.rs`, `tasks.rs`, `gpu_compat.rs`, `scheduler.rs`, `cache_warmer.rs`, and `commands/{auth,backends,backup_plan,browse,cache,crypto,keychain,notify,repo,repo_locks,rest_probe,schedule,snapshot,transfer,webhook}.rs`.
  The plan-tree engine (`plan_rules`, `plan_edit` incl. `plan_edit/tests.rs`, `plan_tree`, `plan_listfiles`) also carries `#[ignore]` differential tests against the real restic (`cargo test restic_ -- --ignored`, also in `restic-compat`) — see docs/backup-plan-tree.md.
- Layout tests live in `tests/layout/` (`run.mjs` + `mock.js` + `pages.html`/`modals.html`): the real production
  build in headless Chrome with the Tauri IPC mocked and long-name data, failing on horizontal scroll, clipped
  controls, wrapped short text or a modal escaping its dialog — see docs/frontend.md ("Layout tests"). Needs a
  local Chrome/Chromium (`CHROME_BIN`); skips without one (fails only with `LAYOUT_REQUIRE_BROWSER`). Deliberately
  not run by CI (`test.yml` calls `test:vite`/`test:rust` separately). Known gap: it never renders the plan tree
  (`tree_supported` is unmocked in `mock.js`) or opens its dialogs, and no committed test asserts the plan editor's
  footer stays in view.
- CI (`.github/workflows/test.yml`) runs on every push that isn't a `v*` tag and on PRs.

```bash
npm run typecheck   # tsc --noEmit (tsconfig has strict + noUnusedLocals/Parameters)
npm run lint         # eslint src (react-hooks rules only, see below)
npm run lint:rust    # cargo clippy --all-targets -- -D warnings
npm run lint:all     # both of the above
npm run test:vite   # frontend tests only
npm run test:rust   # Rust tests only (cargo test)
npm run test:layout # layout tests: real build in headless Chrome, mocked backend (~15s)
npm run test:all    # all three
```

Linting is deliberately narrow and **is wired into CI** — `.github/workflows/test.yml` runs
`npm run lint:all` on `ubuntu-22.04` alongside typecheck and both test suites, so a clippy
warning does fail the build; it's not merely a local-only gate. `eslint.config.js`
enables only `eslint-plugin-react-hooks` (`rules-of-hooks` + `exhaustive-deps`) — no
`typescript-eslint` rule sets, no stylistic rules — because `npm run typecheck` already covers
type errors and stylistic linting adds churn without preventing the regressions this project
actually sees. `npm run lint:rust` runs `cargo clippy` with `-D warnings`; the few call sites that
can't reasonably shrink (`#[tauri::command]`s with many parameters, two intentionally
fire-and-forget `spawn_blocking`s — the post-cancel unlock in `execute_backup`, snapshot.rs, and webhook
delivery in webhook.rs) carry a targeted `#[allow(clippy::...)]` with a comment,
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
