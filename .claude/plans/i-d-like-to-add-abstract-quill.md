# Scheduling / Automations Feature

## Context

Users want to automate their backup plans on a time-based schedule (e.g. "run these backups every night at 2 AM"). Schedules are intentionally separate from backup plans so one schedule can trigger multiple plans, and one plan can appear in multiple schedules.

The scheduler runs as a background Tokio task inside the Tauri process. As long as the app is alive — including when it lives in the system tray (planned future feature) — schedules will fire. This makes in-app scheduling equivalent to tray-based scheduling with no extra work.

## Architecture

### Background Scheduler
A single long-running `tokio::spawn` task in `scheduler.rs` (new file at crate root) wakes every 60 seconds, queries for due schedules (`WHERE enabled=1 AND next_run_at <= now`), and calls the extracted backup function for each plan in each due schedule. It silently skips if the app is locked.

### Key Refactor: extract `execute_backup`
The current `run_backup` Tauri command owns all backup logic inline. Extract it into a free async fn `execute_backup(app, db, master_key, repo_id, plan_id?, paths, tags, excludes)` so both the command and the background scheduler can call it. `run_backup` becomes a thin wrapper — pure refactor, no behavioral change.

### Cron expressions
Use the `cron` crate (v0.12). It expects 7 fields; prefix user 5-field strings with `"0 "` (second=0). The UI builds the expression from friendly fields (daily/weekly/monthly + time), or the user can enter a raw expression in expert mode — mirrors the existing simple/expert exclude-pattern toggle in `BackupPlanEditPage.tsx`. `next_run_at` is pre-computed at save time and after each run, so the hot polling loop is a simple integer compare.

## SQLite Schema

Add to `init_schema` in `cache.rs`:

```sql
CREATE TABLE IF NOT EXISTS schedules (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    plan_ids_json   TEXT NOT NULL,   -- JSON array of backup_plan IDs
    cron_expr       TEXT NOT NULL,   -- e.g. "0 2 * * *" (5-field user input, stored as-is)
    enabled         INTEGER NOT NULL DEFAULT 1,
    last_run_at     INTEGER,         -- unix epoch seconds, NULL if never run
    next_run_at     INTEGER,         -- pre-computed; NULL means never fires (bad expr)
    created_at      INTEGER NOT NULL
);
```

Also add `DELETE FROM schedules;` to `reset_all`.

## Files to Change

| File | Change |
|---|---|
| `src-tauri/Cargo.toml` | Add `cron = "0.12"`, explicit `chrono = "0.4"`, `tokio = { version="1", features=["time"] }` |
| `src-tauri/src/commands/cache.rs` | Add `schedules` table in schema, `reset_all` DELETE, 7 new `AppDb` methods (below) |
| `src-tauri/src/commands/snapshot.rs` | Extract `execute_backup` free async fn; `run_backup` wraps it |
| `src-tauri/src/commands/schedule.rs` | **New** — `Schedule` struct, 5 commands, cron helpers |
| `src-tauri/src/commands/mod.rs` | Add `pub mod schedule;` |
| `src-tauri/src/scheduler.rs` | **New** — `spawn(AppHandle)` + `tick` async fn |
| `src-tauri/src/lib.rs` | Add `mod scheduler`, import `schedule`, call `scheduler::spawn` in setup, add 5 commands to handler |
| `src/lib/types.ts` | Add `Schedule` and `ScheduleFrequency` interfaces |
| `src/lib/invoke.ts` | Add 5 schedule invoke wrappers |
| `src/App.tsx` | Add `/schedules` and `/schedules/:scheduleId` routes |
| `src/components/Sidebar.tsx` | Add Schedules nav item (clock icon) between Backup Plans and Logs |
| `src/pages/SchedulesPage.tsx` | **New** — list view |
| `src/pages/ScheduleEditPage.tsx` | **New** — create/edit form |

## Rust Types & Commands (`schedule.rs`)

```rust
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub plan_ids: Vec<String>,
    pub cron_expr: String,        // stored as 5-field; internally prefixed with "0 "
    pub enabled: bool,
    pub last_run_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub created_at: i64,
}

list_schedules(db) -> Result<Vec<Schedule>, String>
save_schedule(db, schedule) -> Result<(), String>          // validates cron, sets next_run_at
remove_schedule(db, schedule_id) -> Result<(), String>
toggle_schedule(db, schedule_id, enabled) -> Result<(), String>
run_schedule_now(app, db, master_key, schedule_id) -> Result<(), String>
```

Cron helper functions (also in `schedule.rs` or a `cron_utils.rs` submodule):
- `next_fire_time(expr: &str) -> Result<i64, String>` — used at save and after run
- `describe_cron(expr: &str) -> String` — human label, e.g. "Every day at 02:00"

## New `AppDb` Methods (`cache.rs`)

```rust
list_schedules() -> Result<Vec<Schedule>, String>
save_schedule(s: &Schedule) -> Result<(), String>
remove_schedule(id: &str) -> Result<(), String>
set_schedule_enabled(id: &str, enabled: bool) -> Result<(), String>
list_due_schedules(now: i64) -> Result<Vec<Schedule>, String>   // WHERE enabled=1 AND next_run_at<=now
record_schedule_run(id: &str, ran_at: i64) -> Result<(), String> // update last_run_at, recompute next_run_at
get_plans_for_ids(ids: &[String]) -> Result<Vec<BackupPlan>, String>  // dynamic IN clause
```

## TypeScript Types

```typescript
export interface Schedule {
  id: string;
  name: string;
  planIds: string[];
  cronExpr: string;
  enabled: boolean;
  lastRunAt?: number;
  nextRunAt?: number;
  createdAt: number;
}
export type ScheduleFrequency = "daily" | "weekly" | "monthly" | "custom";
```

## Frontend Pages

**`SchedulesPage.tsx`** — mirrors `BackupPlansPage.tsx`:
- List cards: schedule name, humanized cron description, plan count, enabled badge, last/next run
- Per-card: Enable/Disable toggle, Edit, Run Now, Delete
- Delete confirmation modal

**`ScheduleEditPage.tsx`** — mirrors `BackupPlanEditPage.tsx` with simple/expert toggle:
- Simple mode: frequency selector (Daily/Weekly/Monthly) → time picker → optional day selector → assembles cron string
- Expert mode: raw 5-field cron text input with live validation (calls `save_schedule`; shows error if cron is invalid)
- Multi-select plan list (checkboxes over existing backup plans)
- Enabled toggle
- Next 3 runs preview (derive from `nextRunAt` returned after save, or call a lightweight `validate_cron_expr` command)
- `scheduleId="new"` for creation, existing ID for edit

## Implementation Order

1. `Cargo.toml` — add deps
2. `cache.rs` — schema + AppDb methods
3. `snapshot.rs` — extract `execute_backup` (pure refactor, verify existing backup still works)
4. `commands/schedule.rs` + `commands/mod.rs`
5. `scheduler.rs` + wire into `lib.rs`
6. TypeScript: `types.ts`, `invoke.ts`
7. Frontend: `SchedulesPage.tsx`, `ScheduleEditPage.tsx`
8. `App.tsx` + `Sidebar.tsx`

## Pitfalls to Watch

- **Cron field count**: `cron` v0.12 is 7-field; always prepend `"0 "` to user's 5-field string before parsing
- **`IN` clause**: `rusqlite` doesn't bind arrays; build `(?1,?2,...)` dynamically from slice length
- **Locked state**: scheduler tick must silently return if `master_key.get()` is `Err`
- **Concurrent runs**: use a `tokio::sync::Semaphore(1)` or `Arc<AtomicBool>` in `scheduler.rs` to prevent overlapping executions if a plan is slow and the 60s tick fires again

## Verification

1. `npm run tauri dev` — app compiles and launches
2. Create a schedule with 2 plans, set cron to `* * * * *` (every minute), enable it
3. Wait ~60 seconds; verify both plans run and appear in Logs
4. Disable the schedule; verify it no longer fires
5. "Run Now" button triggers plans immediately regardless of schedule
6. Lock the app; verify scheduler tick skips silently (no crash, no "App is locked" notification)
7. Edit schedule in expert mode with an invalid cron string; verify error is shown and schedule is not saved
