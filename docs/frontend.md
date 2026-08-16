# Frontend Reference

Read this before: changing a page's behavior, adding a component, or touching theming.
Per-file detail lifted from the `src/` project-structure tree; CLAUDE.md keeps only a
one-line summary per file. Route table lives in CLAUDE.md.


### `App.tsx`

Router + layout shell; auth state machine (loading/setup/locked/unlocked/ updateNotice); the mount effect tries auto-unlock before ever rendering the password screen — runAutoUnlock() (shared with the "updateNotice" screen's Continue button) calls tryAutoUnlock(), landing on "unlocked" or "locked" with a reason code ("" | "denied" | "stale") mapped to AuthPage's `notice` prop via AUTO_UNLOCK_NOTICES; "updateNotice" (macOS only, gated on autoUnlockNeedsPromptWarning()) warns about the incoming keychain permission dialog before triggering it; startupRanRef guards the whole startup sequence against React StrictMode's dev-only double-invoke of effects, so tryAutoUnlock() — and the real keychain prompt it can cause — only ever fires once per launch even in `npm run tauri dev` (no effect in production, where StrictMode's double-invoke doesn't happen); a `menu:lock-app` listener (while unlocked) calls lockApp() and returns to "locked" without touching the keychain, since locking is a session action, not a settings change; ErrorBoundary catches render errors; restic version warning banner on unlock

### `main.tsx`

React entry; suppresses context menu globally

### `index.css`

Tailwind directives + global styles


## components/

### `ActionButton.tsx`

Canonical row-action button (Edit/Delete/Restore/Browse/Search/etc. inside a table row or card
list): renders an icon plus a text `label`, where the label and a neutral `border-gray-700`
outline are both a pure CSS reveal (`hidden wide:inline` / `wide:border-gray-700`) at the `wide`
Tailwind breakpoint (`tailwind.config.js`, `theme.extend.screens.wide = "1536px"`, well above the
1280 window minimum and typical MacBook logical widths so labels only reveal on a genuinely wide
window) — icon-only
and borderless below it, icon + text + outline above it, no resize listener or re-render on drag
(the border is always present but `border-transparent` below `wide`, so it never shifts layout
when it becomes visible). `label` should stay one word wherever the action allows it (`"Edit"`,
`"Delete"`, `"Restore"`) since it becomes inline button text, not just a tooltip; it's also the
accessible name (`title`/`aria-label`) unless an optional `title` prop overrides it — use that
override to keep a longer, more specific hover string ("Browse files", "Delete snapshot") or a
disabled-state explanation ("This repository is read-only") without lengthening the visible
button text. `tone` (`blue`/`green`/`purple`/`yellow`/`red`) sets the hover color, matching the
per-action colors already established (blue = navigate/search, green = restore/run, purple =
copy/mirror, yellow = retention, red = destructive). Replaces the two icon-only treatments this
project used to have — bare `p-1.5` buttons in tables and `<Button variant="ghost" size="sm">` in
card lists — as the one shared row-action shape; don't reintroduce either at a new call site.
Why a number close to `1280` and not a round physical-resolution number like `1920`: Tailwind breakpoints (like
Tauri's `minWidth`/`minHeight` window config) are in *logical* (CSS) px, which OS display scaling
divides down from physical resolution, so a physical 1920px monitor is very often a
1280-1536px logical viewport (e.g. a MacBook Pro 14" is 1512 logical regardless of its 3024px
panel) — a 1920 breakpoint would rarely or never fire on a laptop's built-in display. Also worth
knowing: because the app's window can never go below `tauri.conf.json`'s `minWidth: 1280`, every
*stock* Tailwind breakpoint below `2xl:1536` is permanently true and therefore useless here —
`wide` is the only breakpoint in this codebase that actually toggles.

### `Button.tsx`

primary/secondary/danger/ghost variants

### `ContextMenu.tsx`

Portal-rendered right-click menu; auto-nudges onto screen; closes on Escape/click-outside

### `EmptyState.tsx`

Empty list placeholder

### `icons.tsx`

Canonical glyph icons — the single source of truth for every icon used in more than one place (TrashIcon, PencilIcon, XIcon, ChevronDownIcon, ChevronRightIcon, CheckIcon, WarningIcon, the solid status set CheckCircle/XCircle/MinusCircle/WarningSolid, SearchIcon, RestoreIcon, LockIcon). Same meaning must always use the same drawing: add a new icon here (or import an existing one) rather than copy-pasting an inline `<svg>` at point of use. Single-use decorative illustrations (e.g. the empty-state search magnifiers with their strokeWidth variation, Sidebar nav glyphs) stay inline in their page. The duplicated per-page `FileIcon`/`DirIcon` pairs remain an accepted, deferred duplication — see docs/decisions.md. Style split: outline-stroke icons for actions (delete/edit/close, banner warnings, inline confirm ticks, select chevrons) and solid-20 icons for compact row actions and modal-header status glyphs. Row-action glyphs (Edit/Delete/Restore/etc. inside a table row or card list) are wrapped in `ActionButton.tsx`, not a bare `<button>` or `<Button variant="ghost" size="sm">` — see that entry for the responsive icon+label pattern.

### `ImportExportCard.tsx`

Settings card: export all repos/plans/schedules to an encrypted .json file, and import (preview→confirm) as fresh copies; import modal tabs between Resty Export and Backrest config.json

### `Input.tsx`

Labeled input with error state; optional onClear prop shows inline × when value non-empty; className applies to outer wrapper div (not <input>); <input> is always w-full inside wrapper

### `Modal.tsx`

Overlay modal dialog

### `ProgressBar.tsx`

Determinate (percent) or indeterminate (constantly-sliding, via index.css's `slide` keyframe) bar; shared by ActivityPanel and every modal that shows backup/restore/copy/index/prune progress — indeterminate mode is for operations that report no incremental progress (e.g. single-repo prune)

### `Sidebar.tsx`

Left nav with app icon + repo indicator; the nav list is routes only (Repositories, Backup Plans, Schedules, Logs, Settings) plus, last, a "Lock" button (same format as the nav items, `onLock` prop — only passed when unlocked, since Sidebar renders solely in App.tsx's unlocked branch) that locks the session via the shared `handleLock` in App.tsx, the same path as the menu bar's and tray's "Lock Now". Below the nav list, separated by the footer border, sits a status strip — a `<button>` carrying `data-activity-toggle`, not a NavLink, deliberately not part of the nav list since it toggles a panel rather than navigating — that toggles the Activity panel via the `activityOpen`/`onToggleActivity` props from App.tsx. While the panel is shown it gets a neutral pressed treatment (`bg-gray-800 text-gray-200` — the hover end-state), deliberately NOT the blue nav-active treatment: "you are here" is route navigation's job, and a blue highlight here would leave two items highlighted at once. The `data-activity-toggle` marker is what ActivityPanel's outside-close handler skips — without it, clicking the toggle while open would close on mousedown and re-open on the subsequent click. The strip renders `activeTaskCount(useActivity())` (lib/activity.tsx's shared counter — the same computation behind the panel's empty-state, so the two can't drift; semantics: queued batches/mirrors count too, statsRefreshAllProgress supersedes statsRefreshing, statsFailed is not activity) as a sentence — "N tasks running" (glyph and text `text-blue-400`/`text-gray-400`, plus a thin indeterminate `ProgressBar`) or "No background activity" (`text-gray-500`) — rather than a separate count chip, so there's only one place activity is reported, not two that could visually drift

### `ActivityPanel.tsx`

Right-side panel surfacing background activity with no other visibility: auto-indexing progress, scheduler-triggered backups (Active Tasks — driven by the unified `task` event bus filtered to origin "scheduler", see Operation Event Bus and lib/activity.tsx's reduceSchedulerBackup; Stop wired to cancelBackup(), which kills whatever's in BackupHandle.child regardless of manual/scheduler origin; shown only during the "backup" phase, hidden during "retention" since apply_retention has no cancel path — subtitle swaps to "Applying retention rules…" so the ~10-20s forget isn't mistaken for a frozen bar), in-flight repo stats refreshes (also in Active Tasks — lifecycle-only, no progress bar), next few due schedules (Upcoming Tasks — rows truncate with a hover tooltip for long plan lists), and last few backup runs (Recent Logs — neutral "Cancelled" glyph instead of red-X/"Failed" for CANCELLED_BACKUP_ERROR entries), and queued/running mirror runs (Active Tasks — one row per mirror, "Up Next" for any additionally queued ones; see lib/activity.tsx's reduceMirror). Restore/copy/manual backup still have their own blocking progress modals and are intentionally excluded — see lib/activity.tsx.

A `fixed inset-y-0 right-0` overlay drawer, slid in/out with a transform (no reflow, no scrim) — always mounted and animated so it slides both in and out, just off-screen + non-interactive when closed. There is no right-edge rail — the Sidebar's footer status strip is the sole thing that opens it, and a chevron button in the panel's own header (or a click outside the drawer) closes it. `open`/`onClose` are owned by App.tsx (passed as props, always closed on launch, never persisted) so the Sidebar's status strip and this panel toggle the same drawer; `onClose` is passed as a stable `useCallback` since the outside-close effect depends on it. That effect skips clicks on `[data-activity-toggle]` (the Sidebar strip) so a toggle-click while open doesn't close-on-mousedown then re-open-on-click. The panel's own Active-Tasks empty-state derives from `activeTaskCount` (lib/activity.tsx), the same shared counter the Sidebar strip renders.


## lib/

### `types.ts`

Shared TS types: Repository, Snapshot, FileEntry, ResticStats, SnapshotStats, CheckResult, BackupHistoryEntry, BackupProgress, RestoreProgress, RetentionPolicy, BackupPlan, DiffEntry, DiffResult, NewRepoInput (mirrors repo.rs's NewRepoInput struct); isRemoteRepo() helper; CANCELLED_BACKUP_ERROR sentinel (see snapshot.rs's execute_backup) distinguishing a genuine cancel from a real failure

### `backends.ts`

detectBackend (mirrors commands/backends.rs's detect_kind — total, never persisted) and commonCredentialKeys (one-line hint naming common env vars for a recognized b2:/s3:/rest: prefix, display-only). The repository path itself is always freeform — there is no per-kind guided form; see RepositoriesPage.tsx below. Also hasBrokenRestUserinfo/hasInlineRestUserinfo (over a shared restUserinfoParts parser, deliberately not `new URL()` since that throws on the exact malformed/ partial input these exist to detect): the first flags a rest: path whose inline userinfo contains a /, @, ?, or # — which breaks Go's net/url the same way restic's own parser breaks (see Restic Integration's REST paragraph) — the second flags any inline userinfo at all, since restic's ApplyEnvironment only reads RESTIC_REST_USERNAME/PASSWORD when the URL has neither, silently ignoring stored credential rows otherwise. Both display-only, consumed by RepositoriesPage.tsx

### `invoke.ts`

Typed wrappers over tauri invoke()

### `activity.tsx`

ActivityProvider (mounted once in App.tsx, outlives route changes since it must keep updating no matter which page is mounted): indexing progress, the scheduler-triggered activeBackup (never a manual/"Run Now" backup — derived from the unified `task` bus filtered to origin "scheduler" via the pure, unit-tested reduceSchedulerBackup reducer, replacing the legacy scheduler:backup-started/backup:progress/ scheduler:retention-started/scheduler:backup-finished events outright — see Operation Event Bus) carrying a phase ("backup"|"retention") flipped by the retention step's own `forget`-kind task op reaching "started"; a plan with no retention configured never gets a `forget` op, so that case is instead dismissed by a plan-lookup effect once it confirms no keep_* flag is set (see reduceSchedulerBackup's doc comment for why the reducer alone can't know this), upcoming due schedules (refreshed on schedules:changed, which the scheduler emits after record_schedule_run advances next_run_at — NOT on the task bus, which fires per-plan before the advance and would read a stale past timestamp), recentLogs, and statsRefreshing/statsFailed — repoId sets derived (via the pure, unit-tested reduceStatsOps reducer, StatsOpsState) from the unified `task` event bus filtered to kind "stats" rather than from a per-operation feed (stats never had one). Lifecycle-only, no error text: the reducer tracks operationId→repoId across started (also clears any prior failure marker for that repo)/finished/failed/cancelled to drive an in-flight indicator (statsRefreshing — a spinning icon on RepositoriesPage's own rows, an indeterminate ProgressBar in the Activity panel) and a plain boolean "last attempt failed" marker (statsFailed, no message — see repo.rs's fetch_and_cache_stats, where every failure path reports through task_ctx.failed(...) explicitly so this marker never depends on the invoke promise's own rejection). The actual numbers are re-read from the DB cache by RepositoriesPage's own `task` listener (only on "finished"), not carried on the event. Powers ActivityPanel.tsx and (for statsRefreshing/statsFailed) RepositoriesPage.tsx directly. activeMirrors tracks every queued/running mirror (Map<operationId, ActiveMirror>, not a nullable slot — mirror_repo allows multiple runs to be queued at once, including two into the same destination from different sources, so it's attributed strictly by operationId, never repoId — see reduceMirror). Mirror has no `progress` phase (restic `copy` streams nothing incremental), so a running row is always an indeterminate ProgressBar. Also exports two pure helpers shared by ActivityPanel and Sidebar: `standaloneSnapshotIndexes` (the batch-suppression filter for standalone per-snapshot index rows — formerly inlined in the panel) and `activeTaskCount` (the number of background tasks the panel surfaces, driving the Sidebar's footer status strip and the panel's empty-state; counts queued batches/mirrors too, renders statsRefreshAllProgress/statsRefreshing exclusively, and ignores statsFailed/upcoming/recentLogs — unit-tested in activity.test.ts).

### `format.ts`

formatBytes, formatSize, formatDate, formatDateOnly, formatTimestamp, formatDuration

### `config.ts`

MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR constants for version warning

### `utils.ts`

needsFullDiskAccess(paths): returns true if any path matches macOS protected prefixes (~/Library, /System, /private, /var)

### `theme.tsx`

ThemeProvider + useTheme(); persists to localStorage; applies dark/light/system class to <html>

### `cron.ts`

parseCronToSimple/buildCronExpr — pure Simple<->Expert cron helpers, moved out of ScheduleEditPage.tsx (where they were module-private) so they're directly unit-tested (cron.test.ts); round-tripping a parsed expression through buildCronExpr is not byte-identical (zero-padded, e.g. "0 2 * * *" -> "00 02 * * *") — intended, pinned by a test, not a bug

### `difftree.ts`

computeChildren/toSegments — pure tree-building over DiffEntry[] for DiffPage.tsx's directory browser, moved out the same way (where they were module-private) for the same reason (difftree.test.ts)


## pages/

### `AuthPage.tsx`

Master password setup (first launch) and unlock screen

### `RepositoriesPage.tsx`

Add/open/delete repos; restic init for new repos; remote URL support; add/open modal keeps the plain Local Path / Remote URL toggle (folder picker vs. a freeform restic-URL text input — no per-backend guided sub-forms or Backend `<select>`; backend kind is only ever derived from the path, never chosen or stored) plus, for Remote, one universal optional "Credentials (optional)" key/value row list (env var name + value, add/remove rows) — the same shape for every backend, since restic's own interface is env vars regardless of which one. Left empty, a repo uses restic's own credential chain (env, `~/.aws/credentials`, an IAM role) exactly as it always has; a one-line hint (`lib/backends.ts`'s `commonCredentialKeys`) names common env vars (e.g. `B2_ACCOUNT_ID, B2_ACCOUNT_KEY`) once the typed path matches a recognized `b2:`/`s3:`/`rest:` prefix. For `rest:` specifically, two more advisory hints (`hasBrokenRestUserinfo`/`hasInlineRestUserinfo`, `lib/backends.ts`) appear under the path field in both add and edit modals: an amber one when inline URL userinfo contains a character (`/`, `@`, `?`, `#`) that breaks restic's URL parsing (only shown when true — a working URL never has both conditions), and a neutral one — shown only when the amber one isn't — when the URL has *any* inline userinfo and a credential row is filled in, since restic silently prefers the URL's own credentials over `RESTIC_REST_USERNAME`/`PASSWORD` in that case (see Restic Integration's REST paragraph). Neither hint blocks Save or Test Connection. Rust's `validate_credentials` (`backends.rs`) is the sole authority on required/allowed keys per detected kind — this form does no client-side required-field validation, so a missing/unknown key surfaces as a Test Connection / Save error rather than an inline warning; the Remote-mode path is trimmed before submit/test (`handleSubmit`/`handleTest`) since it's the only freeform text field here — the Local-mode path comes verbatim from the folder picker and is never trimmed, since a trailing space can be a real part of a directory name; credential values are trimmed on submit except `RESTIC_REST_PASSWORD`, where whitespace can be part of the password itself; the edit modal derives backend kind from the (possibly just-edited) path via `detectBackend` rather than a stored field, and blocks Save with an inline error if editing the path would change the derived kind while credentials are already stored; edit-modal credential rows are prefilled with stored key+value pairs from `getRepoCredentials` (same threat model as `getRepoPassword` — values round-trip the same way the password does); editing one row leaves the others intact, and "Clear stored credentials" empties the row list (saved as an empty list → ambient mode); Test Connection uses the populated values directly, so it reflects the saved repo's actual credentials; "Read-only repository (--no-lock)" checkbox in the add modal (next to the existing "No Password" checkbox) and the edit modal (dirty-checked like name/path/password, via updateRepoReadOnly); a "Read-only" pill badge on the row; read-only repos are excluded from every mirror/copy destination picker (never as a source) and "Prune…"/row Mirror button are disabled accordingly (see Restic Integration for the backend policy); per-row and bulk stats refresh (manual-only — no auto-eviction; see Restic Integration; "Refresh All" always includes remote repos, unlike every automatic remote activity); the row's stats block is formatted by `lib/format.ts`'s `formatRepoSize` — when `raw_size` (on-disk stored size) is present it's promoted to the bold headline figure with the restore-size (`total_size`) folded into the existing secondary line next to the snapshot count, plus a hover tooltip spelling out both numbers and the derived compression+dedup space-saving percentage; a legacy cache row or a cycle where the raw-data call failed (`raw_size` null/absent) falls back to today's single-size layout with no tooltip — no fourth line is ever added, and no new UI state (spinner/failure marker) exists for this beyond what the existing stats refresh already drives; spinner (statsRefreshing) and failure marker (statsFailed, a plain boolean — no error text, see activity.tsx) both come from ActivityProvider's `task`-bus subscription and survive navigating away mid-refresh; row data comes from a page-local `task` listener re-reading get_repo_stats on "finished" (a guaranteed cache hit); each row shows a "Refreshed …" label from cached_at, and a failed refresh keeps the last-good value visible with a plain "refresh failed" marker rather than blanking to "unavailable"; mirror, edit, check, prune, "Index All Snapshots" via right-click context menu; edit modal: name/path/password with Test Connection; prune: confirmation→progress→done, with a Hide button on the progress screen (mirrors SettingsPage's "Prune All Repositories" modal) that dismisses the modal while the prune keeps running — reopening via "Prune…" shows the same repo's live progress rather than a blank state, sourced from local `pruning`/`pruneElapsed`; "Prune…" is disabled for every repo except the one currently pruning (prune is single-in-flight app-wide, `PruneHandle`'s busy guard) so a click on a different repo can't silently reopen the modal onto the wrong repo's progress; a backgrounded prune otherwise stays visible/cancellable via the Activity panel's `activePrune` row — see ActivityPanel.tsx; mirror: destination picker → queued/running (an indeterminate bar; mirror_repo emits no progress) → done/cancelled/failed, with a Hide button on the queued/running screen that backgrounds the run without cancelling it (mirrorOpId/mirrorOutcome tracked locally; the queued-vs-running state itself is read live from ActivityProvider's activeMirrors, matched by operationId — never repoId, since mirror_repo allows multiple runs queued at once, including two into the same destination from different sources); deliberately no re-adoption path (unlike "Index All"'s getActiveIndexBatch below) — reopening the modal for a repo always starts a fresh picker, since a backgrounded mirror stays fully visible/cancellable via the panel regardless, and the backend's `(src, dest)` dedup guard rejects an accidental exact repeat (MIRROR_ALREADY_ACTIVE_ERROR); a page-local `task` listener drives the terminal outcome, backstopped by a second effect that infers "done" if activeMirrors drops the operationId first (closes a narrow race where Tauri's async `listen()` registration can lose to an unusually fast mirror's terminal event — activeMirrors isn't subject to this, since ActivityProvider has been subscribed since app launch). The backstop requires seen-then-disappeared (mirrorSeenRef), not bare absence: mirror_repo emits its "pending" task event from *inside* the spawned task (snapshot.rs), which lands strictly after the operationId has already returned to the frontend, so activeMirrors provably doesn't contain a just-started run for at least one render — concluding "done" from that absence alone (the original implementation) read the startup gap as a finish and reported success on a mirror that hadn't even begun. Accepted trade-off: a mirror fast enough to never be observed at all now leaves the modal on "Copying snapshots…" instead of resolving, rather than falsely reporting complete — the modal's Hide button and the Activity panel's own independent tracking make that an acceptable stall, not a dead end. mirrorSeenRef is reset alongside every setMirrorOpId(null) (both the row button and the context-menu item) so it never leaks into the next mirror run; "Index All Snapshots" opens the same dismissible progress/queued/Stop/complete modal pattern as RepoSearchPage's own "Index All" (independent state, its own `task` listener scoped to whichever repo the context menu targeted — deliberate duplication, see "Known, deferred frontend duplication" below), and calls the same index_snapshots_batch/getActiveIndexBatch/cancel_index_batch commands, so a batch started from either page is visible in both (and in ActivityPanel) and adopted rather than duplicated; the menu item is disabled per-repo via a page-local `repoNeedsIndexing` map (cache-only listSnapshots+get_snapshot_index_status reads, recomputed on repo-list changes and kept live via the `task` bus + snapshots:refreshed — fails open/enabled while unchecked)

### `SnapshotsPage.tsx`

Snapshot table; stale-while-revalidate cache; inline tag editor; delete with prune option; full-snapshot restore with streaming progress; per-snapshot copy with cancellation; pagination (PAGE_SIZE=10); filter with × clear; right-click context menu; multi-select mode: bulk delete and copy with progress bars; per-row "Index Snapshot" / "Remove Index" context-menu item toggles based on index status: shows "Index Snapshot" (disabled while in_progress) or "Remove Index" (active when complete); "Remove Index" calls clear_snapshot_index and removes the snapshot from the local status map; "Index Snapshot" shows a progress modal; listens for `task` events (kind "index") to update per-row status map live; listens for snapshots:refreshed to reload list when warmer updates cache; per-row and context-menu "Search Files" button → SearchPage; a "Read-only" pill badge next to the repo name when repo.readOnly; delete, tag add/remove, Unlock, and bulk delete are disabled (with a tooltip) for a read-only repo — restore/browse/search/check/refresh stay enabled since they're reads; the copy-destination list (row button, context item, bulk "Copy selected") is filtered to writable repos only via the otherRepos memo (a read-only repo may be a copy *source*, never a destination — see CLAUDE.md's Restic Integration section)

### `BrowsePage.tsx`

File tree inside a snapshot; per-entry and multi-select restore; breadcrumb nav; restore modal with strip_leading_path option; inline tag management (add/remove disabled, with a tooltip, when repo.readOnly — restore itself stays enabled since it writes to the local filesystem, not the repo); a "Read-only" pill badge next to the repo name when repo.readOnly; "Search" button navigates to SearchPage, passing returnPath+returnStack so back navigation can restore the current directory depth; accepts initialPath+initialPathStack from SearchPage so "open in browser" lands at the right directory; fromSearch flag in location state changes back-button destination (navigate(-1) restores search state from history entry written by window.history.replaceState)

### `SearchPage.tsx`

Full-text file search within a single snapshot at /snapshots/:repoId/:snapshotId/search; requires snapshot to be indexed (browse_cache_files); shows index state machine (loading→not_indexed→indexing→ready); "Index Now" triggers index_snapshot; listens for `task` events (kind "index") to transition to ready; debounced 300ms search via search_snapshot_files (SQLite LIKE, capped at 200 results); clicking a result writes restoredQuery+restoredResults into current history entry via window.history.replaceState before navigating to BrowsePage (so navigate(-1) restores them); back button (fromBrowse) navigates explicitly to BrowsePage with returnPath+returnStack to restore the correct directory depth; searchSeqRef guards against out-of-order responses — a burst of keystrokes can have several (slow, ~1s+) searches in flight, so only the response matching the latest call is applied to state

### `RepoSearchPage.tsx`

File search across every indexed snapshot in a repo at /snapshots/:repoId/search; same index/debounce/stale-response-guard pattern as SearchPage.tsx, but backed by search_repo_files, which dedups each matching path to the newest snapshot containing it (shown as a snapshot short-id badge per result; clicking opens that snapshot's BrowsePage). Banner shows "Searching N of M snapshots" with an "Index All" action when the repo is only partially indexed; "Index All" calls index_snapshots_batch once (backend indexes sequentially, one snapshot at a time, pausing the auto-indexer for the run — see browse.rs); a modal with a real progress bar (derived from `task` events, kind "index", matched against the batch's target snapshot ids via targetId) tracks the run, with a Stop button (cancel_index_batch; takes effect between snapshots) shown while in progress; the batch also survives the modal being dismissed — see ActivityPanel.tsx

### `DiffPage.tsx`

Diff viewer at /snapshots/:repoId/diff/:snapshotA/:snapshotB; client-side tree from flat entries; summary bar; restore from diff; truncation warning

### `BackupPlansPage.tsx`

List/run/delete plans; backup modal with streaming progress + cancellation (cancelling shows a local "Stopping…" state, then reverts to the Start Backup view — no distinct "cancelled" UI block, matching cancel_backup's own behavior); auto-applies retention after successful backup; per-plan Apply Retention button; pre-flight FDA check before running: warns if plan includes protected paths and FDA not granted (macOS only); Run Backup / Apply Retention Rules (row buttons and context items) disabled, with a tooltip, for any plan whose repo is currently read-only (planRepoReadOnly) — execute_backup would refuse it anyway (see Restic Integration), this just avoids the round-trip

### `BackupPlanEditPage.tsx`

Create/edit plan (name, repo, paths, tags, excludes, exclude-if-present marker files, exclude-caches, retention, bandwidth limits); exclude patterns: Simple tab (tag list + presets) / Expert tab (freeform textarea); a separate "Exclude If Present" card (flat filename tag list, no Simple/Expert split — restic's `name:header` syntax passes through the plain filename field with no dedicated header input) plus an "exclude cache directories" checkbox (`--exclude-caches`, restic's shorthand for `--exclude-if-present CACHEDIR.TAG:Signature: 8a477f597d28d172789f06886806bc55`); a live, non-blocking amber hint appears under that field's input while it contains a `/`/`\` (marker files match by name only — a path never matches) or starts with `#` (silently dropped by build_exclude_args' comment filter, same as Exclude Patterns) — purely advisory, doesn't block Add, mirrors the pattern's own filtering in snapshot.rs rather than introducing new validation rules; amber FDA warning suppressed when FDA is confirmed granted (macOS only); the repo <select> excludes read-only repos, except the plan's own already-selected repo (kept visible but disabled, "(read-only)" suffix) so an existing plan whose repo has since become read-only isn't silently dropped from the list — shown with an inline amber warning below the select in that case; a "Webhooks (optional)" card after Bandwidth Limits is a **read-only list** — each row uses the RepositoriesPage repo-card shape one nesting level down (bordered rounded-xl `bg-gray-800 border-gray-700` row, hover border): endpoint URL as the `text-sm font-medium` primary line (truncated), provider as a Read-only-style `[10px]` uppercase chip beside it, trigger summary "On started, completed, failed" as the `text-xs text-gray-500` secondary line (amber "No stages selected — never fires." when empty), and vertically-centered ghost-Button glyph actions (PencilIcon edit, XIcon delete) right-aligned — with an "+ Add Webhook" button top-right of the card header. All configuration happens in an Add/Edit Webhook `<Modal>` (same RepositoriesPage form-modal conventions): URL, payload-format select (Generic JSON/Discord/Slack/Teams/Custom JSON), stage checkboxes (defaults completed+failed), and for Custom a placeholder-list hint (`{eventName}` `{repoName}` `{planName}` etc.) plus an editable JSON-body textarea pre-filled with a working default template pinned by a Rust test so the two can't drift. The modal always renders a request-body preview (no toggle): `previewWebhook` — Rust renders the real `build_body` with sample values, so the preview can never drift from what's POSTed — one `<pre>` per *selected* stage, re-fetched when the draft's provider/template changes (seq-ref guards out-of-order responses), an amber note for unknown `{placeholders}` (typos — sent literally at fire time), and a red parse error for a template that doesn't render valid JSON, which also blocks the modal's Save (an invalid custom template can't be committed — including while the preview refetch is still in flight, since `webhookPreviewError` would otherwise hold the previous draft's verdict; commitWebhook bails with a "Validating template" notice until it resolves). The modal has **one** message slot (`webhookNotice`, the app's banner style): Save's URL-prefix/empty-template/parse validation failures and Send Test's ok/err result (`testWebhook` on the current draft, button left of the Cancel/Save footer) share it, so a new message always replaces the old and never stacks; any draft-field edit (URL, format, stage, template) or re-opening the modal clears it. Plan-save keeps one page-level guard for rows that can't have come from the modal — a bad URL prefix from a hand-edited import bundle; see webhook.rs in docs/backend.md for the delivery semantics

### `SchedulesPage.tsx`

List schedules; toggle/delete/run; amber warning when tray disabled; a second amber warning per schedule row when any of its plans (via planIds → BackupPlan.repoId) targets a read-only repo — that plan's backup will fail on every run, the rest of the schedule is unaffected (scheduleHasReadOnlyRepo; loads listBackupPlans + listRepos once on mount for this check)

### `ScheduleEditPage.tsx`

Create/edit schedule (name, cron expr, backup plans); scheduleId="new" for creation; each plan row in the picker shows a "Read-only repo" badge when its repo is read-only; selecting one shows an amber warning below the picker (scoped to currently-*selected* plans via selectedReadOnlyPlans, not just any read-only-badged plan in the full list) stating that plan's backup will fail, not the whole schedule; "Delete Schedule" is a danger-variant Button (not a bare link) on the right of the Save/Cancel row, matching BackupPlanEditPage's layout

### `LogsPage.tsx`

Backup history log; paginated (PAGE_SIZE=10); expandable error rows (only for a real failure — a CANCELLED_BACKUP_ERROR entry renders a neutral "Cancelled" glyph instead of the red error icon, and isn't expandable)

### `SettingsPage.tsx`

Theme selector; tray + auto-indexing + remote-auto-refresh toggles; restic binary path; a launch-at-login toggle (its own pt-4 border-t row, a peer of the other toggles, not visually nested under tray) that's always rendered but disabled/greyed (checked state forced to false) unless the tray toggle is on, since without the tray closing the window quits the app; its description text switches on `autoUnlockSupported && autoUnlock` — "starts hidden in the tray" when both are on (matching lib.rs's `should_start_hidden`), otherwise "opens to the unlock screen"; handleTrayToggle clears launch-at-login on BOTH tray transitions (not just disabling) so it never inherits a surviving OS entry or leaves an orphaned one; backed by the real OS autostart entry (no app_settings row — see Stack table's "Launch at login" row and repo.rs); compression selector; default restore path; prune all repos with streaming progress (read-only repos are excluded — repo.rs's prune_all_repos skips them rather than failing the batch; the confirm/done screens fetch and display the excluded count via listRepos() so that's disclosed rather than silent); import/export card (ImportExportCard); cache management: "Clean Orphaned" (remove stale rows) + "Clear All Cache" (wipe + VACUUM); DB size display (app_data.db + WAL) refreshes after each cache operation; Full Disk Access card (macOS only): green when granted, amber with instructions + Re-check when not; an "Unlock automatically at startup" toggle directly below launch-at-login, shown only when getAutoUnlockSupported() is true (macOS/Windows — never rendered on Linux) and deliberately **not** gated on tray/launch-at-login the way that toggle is gated on tray (see docs/decisions.md for why); on a failed disable the toggle stays off rather than reverting, since set_auto_unlock always clears the row server-side even when the underlying keychain delete fails; a "Notifications" card (below the tray/auto-indexing Toggles card) holds four plain checkboxes — Backup started, Success (files changed), Success (no files changed), Failures — in a `grid-cols-3` layout (row-major: Started/Success-changed/Success-unchanged on row 1, Failures alone on row 2) (deliberately checkboxes, not switches, to keep four categories compact rather than a tall stack of bordered toggle rows like the card above it; Failures carries its clarifying text as a native `title` tooltip instead of a visible description line, for the same space reason; each checkbox uses the app-wide `w-4 h-4 accent-blue-500` + `cursor-pointer` label treatment); backed by a single `NotificationSettings` object (`get_notification_settings`/`set_notification_settings`) rather than four separate settings, since `execute_backup` (Rust) reads all four together on every run. There is deliberately no "Warnings" category — see CLAUDE.md's Settled decisions for why. `updateNotifications` merges against a `notificationsRef` mirror (deliberately **not** a functional `setNotificationsLocal` updater — `React.StrictMode`, on in `main.tsx`, double-invokes updater functions in dev, which would fire the IPC save twice per click) and hands the actual save off to `runNotificationsSaveLoop`, which serializes saves — at most one `setNotificationSettings` call in flight at a time, always sending the latest merged state, looping again if another change arrived mid-save (tracked via `notificationsDirtyRef`). Saves are full-object writes, so a local per-field revert on failure can't be made correct (a later save's payload can already embed an earlier save's change); on failure the loop instead resyncs the whole card from `getNotificationSettings()`, which is correct regardless of what any other save did. The mount-effect's initial `getNotificationSettings()` load is itself guarded against this same class of race — it's discarded if `notificationsTouchedRef` (a one-way latch set the first time `updateNotifications` runs and never cleared) shows the user already made a change before it resolved, since applying a stale fetch over an in-flight or already-optimistic change would silently drop the other three categories back to their last-loaded values (deliberately not the transient `notificationsDirtyRef`/`notificationsSavingRef`, which both return to false once the save loop drains — a slow initial fetch resolving after a completed save would still pass that guard; this specific guard exists only for this card — every other toggle on the page has the same unconditional `getX().then(setXLocal)` load pattern, but a single-field toggle reverting to its last-known value on this race is far lower-stakes than this card's full-object save clobbering three untouched fields). These gate only the desktop notification itself — Recent Logs and the Activity panel are unaffected by any of the four checkboxes

## Theming

Three modes: Dark (default), Light, System. Stored in `localStorage`; applied as `dark`/`light`/`system` class on `<html>`.

All theme-sensitive colors route through CSS custom properties in `src/index.css`. Extended in `tailwind.config.js`:
```
gray.50–950, blue.300/400/700/900, green.300/400/700/900, red.300/400/700/900, amber.300/400/500/700/900
```
`:root` = dark defaults. `html.light` and `@media (prefers-color-scheme: light) html.system` override with light palette (slate family, reversed). Each of the three blocks also sets the CSS `color-scheme` property (`dark`/`light`/`light` respectively) — without it, UA-painted native controls (`<select>` popups, checkboxes, scrollbars) default to light chrome regardless of the app theme, since `color-scheme` isn't inferred from the `--tw-*` custom properties. Any new theme block must set it too.

### Adding a themed color
1. Add `--tw-<color>-<shade>: <R> <G> <B>;` to `:root` and `html.light` (and the `system`
   media-query block — all three must stay in sync, `light` and `system`'s light branch use
   identical values) in `src/index.css`.
2. Extend `tailwind.config.js` under `theme.extend.colors`.
3. Use `text-<color>-<shade>` / `bg-<color>-<shade>` as usual.
4. **Verify contrast in light mode, not just that it compiles.** A shade left out of the
   `:root`/`html.light` pair silently falls through to Tailwind's raw default value — tuned
   for a dark background — in *every* theme, including light. This is exactly what happened
   with `amber-400`/`amber-500`: `amber-300`/`700`/`900` were mapped, but warning text using
   the (very common) `text-amber-400`/`text-amber-500` classes rendered as a pastel amber
   (~1.6:1 contrast) directly on a white page background — invisible in light mode. Fixed by
   mapping `amber-400`/`amber-500` to the same darkened accent already used for `amber-300`
   in light mode (`146 64 14`, ~7.1:1) — the same "collapse related shades to one corrected
   accent value" trick `blue-300`/`blue-400` already use (both map to `29 78 216` in light
   mode). When adding a *new* shade of an already-mapped color, do the same: reuse the
   existing light-mode value for that hue rather than leaving the shade unmapped.

### Text contrast rules (both themes)

Bare text on a page/card background never goes darker than `text-gray-500` — gray-600/700 are
border/divider colors, and in dark mode they render at ~2:1/1.4:1 contrast (invisible; this was
a reported user issue). Both gray-500 values are deliberately *not* stock Tailwind values:
dark `--tw-gray-500` is `139 148 163` (stock: `107 114 128`) and light is `88 103 126`
(stock-ish slate: `100 116 139`), each tuned so gray-500 text passes WCAG AA (≥4.5:1) on every
surface it sits on — dark and light both cover gray-950/900/800 (page/panel/input). Light-mode
`green-400` reuses `green-300`'s value for the same reason (`21 128 61` only held 4.07:1 against
light `gray-800`). These are enforced by `src/lib/contrast.test.ts`, which parses all three
blocks of `src/index.css` and asserts the text-capable shades (gray 100–500, accent 300–500)
hold ≥4.5:1 against the app's surfaces in both themes — run `npm run test:vite` after any
palette edit; the test will name the exact shade/surface pair that regressed.

### Hardcoded colors to avoid
- `text-white` on gray backgrounds → use `text-gray-50` (remaps to near-black in light mode).
- `hover:text-white` on interactive elements → use `hover:text-gray-50`.
- `bg-red-700` for buttons → theme-mapped, becomes pastel pink in light mode. Use `bg-red-600 hover:bg-red-800`.
- Colors outside the extended set (`blue-500/600`, `red-500/6/8`, `yellow-*`) are NOT theme-mapped — intentional for colored-background elements like primary/danger buttons where white text is always on a dark surface, where the surface itself (not the page background) sets the contrast context.
- Amber/red/green/blue text used **without** a colored box behind it (a bare warning line, an
  inline status label) must use a mapped shade (`amber-300/400/500`, `red-300/400`, `green-300/400`,
  `blue-300/400`) — never an unmapped shade like `amber-600` or `red-500` — since that text sits
  directly on the page background, which flips between near-black and white across themes.

