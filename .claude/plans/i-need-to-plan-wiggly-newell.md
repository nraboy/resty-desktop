# Task Activity Panel

## Context

Resty Desktop does a fair amount of work **invisibly in the background**: the 60s
`cache_warmer` tick refreshes snapshot metadata and (when `auto_indexing` is on) pre-indexes
file trees one snapshot at a time, and the `scheduler` fires backups on cron with no window
open. Today none of this surfaces — the user can't tell how far along indexing is, when the
next scheduled backup runs, or how the last few runs went without navigating to Logs/Schedules.

A user suggested (mockup provided) a **Task Activity** panel giving at-a-glance status. Per the
user's refinement, the panel should focus on **background / non-user-initiated activity**, not
user-triggered operations (restore, copy, mirror, manual backup) which already have their own
progress modals. Scope:

- **Active Tasks** — background auto-indexing progress + a currently-running *scheduled* backup.
- **Upcoming Tasks** — the next couple of enabled scheduled backups.
- **Recent Logs** — the last couple of backup history entries (success/failure).

Delivered as a **collapsible right-side drawer** (persistent across page navigation), toggled
from a button, open/closed state saved to `localStorage`.

## Design overview

The drawer is a third flex sibling in the app shell (after `Sidebar` and the main column).
Because it must stay live regardless of the current route, its data-gathering listeners are
**hoisted to a top-level `ActivityProvider`** rather than living in a page (page-local listeners
are torn down on unmount — see `SnapshotsPage`/`BackupPlansPage`). Existing page listeners are
left untouched; Tauri events broadcast, so the new top-level listeners coexist with them.

Almost everything is derived from **existing** commands/events. The only backend change is two
lightweight lifecycle emits in the scheduler (to distinguish a *background* scheduled backup from
a manual one, which we deliberately do NOT show), plus one small aggregation command for index
progress.

## Backend changes (`src-tauri`)

### 1. Scheduler lifecycle events — `src-tauri/src/scheduler.rs`
In `tick`, around the per-schedule `execute_backup` call (currently ~lines 99–100), emit:
- `scheduler:backup-started` — payload `{ scheduleName: String, planName: String }` before the run.
- `scheduler:backup-finished` — payload `{ success: bool }` after the run.

`execute_backup` already emits `backup:progress` globally during the run, so the panel fills the
progress bar from those while a `scheduler:backup-started` task is active (the `BackupHandle`
busy flag guarantees only one backup runs at a time, so progress maps unambiguously to the active
scheduled task). Manual backups (`run_backup` from `BackupPlansPage`) emit no `scheduler:*`
events, so they correctly stay out of the panel. Use the same `app_handle.emit(...)` pattern as
`cache_warmer.rs:102` / `snapshot.rs:369`.

> Note: `run_schedule_now` (user pressing "Run" on SchedulesPage) is user-initiated — leave it
> out (do not emit `scheduler:*` there) so it behaves like a manual backup.

### 2. Aggregate index progress — `src-tauri/src/commands/browse.rs`
Add `#[tauri::command] async fn get_index_progress(...) -> Result<IndexProgress>` returning
`{ cached: u64, total: u64 }` summed across all eligible repos (respect `remote_auto_refresh`
the same way the warmer does). Implement by counting `browse_cache_status.status = 'complete'`
vs. total cached snapshots — ideally one SQL aggregate in a new `AppDb` method
(`cache.rs`), run via `tauri::async_runtime::spawn_blocking` per the Persistence & Caching rule
(DB work off the core async workers). This gives the panel a single cheap call instead of
iterating repos with `listSnapshots` + `getSnapshotIndexStatus` on the frontend.
- Register in `invoke_handler!` in `src-tauri/src/lib.rs`.

## Frontend changes (`src`)

### 3. Types + invoke wrappers
- `src/lib/types.ts`: add `IndexProgress { cached: number; total: number }`; a UI-only
  `ActiveTask` shape is fine to keep inside the provider.
- `src/lib/invoke.ts`: add `getIndexProgress(): Promise<IndexProgress>`. Reuse existing
  `listSchedules`, `listBackupPlans`, `listBackupHistory`, `getAutoIndexing`, `describeCronExpr`.

### 4. Relative-time formatter — `src/lib/format.ts`
Add `formatRelative(ts: number): string` → "in 3 hours", "in 4 days", "10 min ago", "1 hour ago"
(mockup wording). Reuse existing `formatTimestamp`/`formatDuration` where possible. Add a small
unit test in `src/lib/format.test.ts` (Vitest, matches existing test convention).

### 5. `ActivityProvider` — new `src/lib/activity.tsx`
A context provider (mounted in `App.tsx` inside the unlocked `<BrowserRouter>`, alongside
`MenuEventHandler`) that owns all panel state and exposes it via `useActivity()`:

- **Active — indexing:** on mount + on every `index:done` and `snapshots:refreshed` event, and
  only when `getAutoIndexing()` is true, call `getIndexProgress()`; expose
  `{ cached, total, remaining }`. Render as an active task only when `remaining > 0`.
- **Active — scheduled backup:** listen to `scheduler:backup-started` (store name/plan),
  `backup:progress` (fill percent/files), `scheduler:backup-finished` (clear the card, then
  refresh Recent Logs). Reuse the `BackupProgress` type already in `types.ts`.
- **Upcoming:** load `listSchedules()` + `listBackupPlans()`; filter `enabled && nextRunAt`,
  sort ascending by `nextRunAt`, take 2; resolve plan names via `planIds`. Refresh on an interval
  (e.g. 30–60s) and after `scheduler:backup-finished` (next_run_at advances after a run).
- **Recent logs:** `listBackupHistory()` (already newest-first), take 2; status derived from
  `error` being present (green check vs. red alert), matching `LogsPage`. Refresh after
  `scheduler:backup-finished`.

Clean up every `listen` unlisten and interval on unmount (follow the `finally`/ref patterns in
`BackupPlansPage`/`SettingsPage`).

### 6. `ActivityPanel` drawer — new `src/components/ActivityPanel.tsx`
Presentational, consumes `useActivity()`. Three sections mirroring the mockup: **ACTIVE TASKS**,
**UPCOMING TASKS**, **RECENT LOGS**, each with an `EmptyState`-style placeholder when empty
(reuse `src/components/EmptyState.tsx` or a lightweight inline muted line). Progress bars reuse
the existing inline pattern (`<div style={{ width: `${pct}%` }}>` as in BackupPlansPage). Use
themed colors per CLAUDE.md (`text-gray-*`, `green.400`, etc. — no hardcoded `text-white`).

### 7. Shell wiring + toggle — `src/App.tsx`
- Wrap the unlocked shell in `<ActivityProvider>`.
- Add `<ActivityPanel />` as a third sibling inside
  `<div className="flex h-screen w-screen ...">`, after the `flex-1` column:
  `<aside className="w-80 flex-shrink-0 border-l border-gray-800 ...">` (mirrors `Sidebar`'s
  left rail). Collapsed → render nothing (or a thin rail); open/closed persisted to
  `localStorage`.
- Add a toggle button. Simplest: a small header/toolbar button rendered at the top-right of the
  main column (the app has no global top bar today — a compact floating/inline toggle in the
  main column's top edge avoids restructuring every page). Persist state so it survives reloads.

## Files touched (summary)
- New: `src/lib/activity.tsx`, `src/components/ActivityPanel.tsx`.
- Edit: `src-tauri/src/scheduler.rs`, `src-tauri/src/commands/browse.rs`,
  `src-tauri/src/commands/cache.rs`, `src-tauri/src/lib.rs`, `src/App.tsx`,
  `src/lib/types.ts`, `src/lib/invoke.ts`, `src/lib/format.ts` (+ `format.test.ts`).

## What is intentionally NOT in scope
- Restore, copy, mirror, manual backup, and prune are user-initiated and already have modals —
  they do **not** appear in the panel (per user's refinement).
- No new persistent "activity history" table; Recent Logs reuses `backup_history`.

## Verification
1. `npm run test:rust` and `npm run test:vite` (covers new `get_index_progress` DB method logic
   and `formatRelative`).
2. `npm run tauri dev`, unlock, confirm the drawer toggles open/closed and the state persists
   across a reload.
3. **Indexing:** enable Auto-indexing (Settings) with an un-indexed repo present; confirm the
   ACTIVE TASKS section shows indexing progress that advances as the warmer sweeps (progress
   updates on `index:done`), and disappears when everything is cached.
4. **Scheduled backup:** create a schedule due in ~1 minute (or temporarily shorten), confirm
   UPCOMING TASKS lists it with a relative time, then when it fires confirm a live ACTIVE TASKS
   card with a moving progress bar, and afterward a fresh entry at the top of RECENT LOGS.
5. Confirm a **manual** backup (Run on BackupPlansPage) still shows its modal but does **not**
   appear in the panel's Active Tasks.
6. Navigate between pages mid-backup to confirm the panel keeps updating (listeners are hoisted,
   not page-local).
