use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{Emitter, Manager};

use crate::commands::cache::{AppDb, BackupHandle, MasterKey};
use crate::commands::repo_locks::RepoLocks;
use crate::commands::schedule::next_fire_time;
use crate::commands::snapshot::{apply_retention, execute_backup, log_retention_failure};

pub fn spawn(app: tauri::AppHandle) {
    let running = Arc::new(AtomicBool::new(false));
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

            if running.load(Ordering::SeqCst) {
                continue;
            }
            running.store(true, Ordering::SeqCst);

            let app_clone = app.clone();
            let running_clone = Arc::clone(&running);
            tauri::async_runtime::spawn(async move {
                tick(&app_clone).await;
                running_clone.store(false, Ordering::SeqCst);
            });
        }
    });
}

async fn tick(app: &tauri::AppHandle) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let db = app.state::<AppDb>();
    let master_key = app.state::<MasterKey>();
    let backup_handle = app.state::<BackupHandle>();
    let repo_locks = app.state::<RepoLocks>();

    // Skip silently when app is locked
    if master_key.get().is_err() {
        return;
    }

    // A backup (manual or scheduled) is already running. Skip this tick entirely
    // without recording the schedules as run, so they stay due and retry on the
    // next tick rather than being silently advanced past. The compare_exchange in
    // execute_backup remains the actual guard against the race; this is the clean
    // early-out so a collision doesn't drop a scheduled occurrence.
    if backup_handle.busy.load(Ordering::SeqCst) {
        return;
    }

    let due = match db.list_due_schedules(now) {
        Ok(v) => v,
        Err(_) => return,
    };

    for sched in due {
        let plans = match db.get_plans_for_ids(&sched.plan_ids) {
            Ok(p) => p,
            Err(_) => continue,
        };

        for plan in plans {
            // These events are only emitted from this background scheduler tick, never
            // from a user-initiated run (manual "Run" on BackupPlansPage, or "Run Now" on
            // SchedulesPage) — that's the signal the Activity panel uses to distinguish a
            // background scheduled backup (which it surfaces) from a manual one (which
            // already has its own progress modal and should stay out of the panel).
            let _ = app.emit(
                "scheduler:backup-started",
                serde_json::json!({ "scheduleName": sched.name, "planName": plan.name }),
            );

            let ok = execute_backup(
                app,
                &db,
                &master_key,
                &*backup_handle,
                &repo_locks,
                &plan.repo_id,
                Some(plan.id.as_str()),
                plan.paths.clone(),
                plan.tags.clone(),
                plan.excludes,
                plan.limit_upload,
                plan.limit_download,
            )
            .await
            .is_ok();

            if ok {
                if let Some(r) = &plan.retention {
                    if r.keep_last.is_some()
                        || r.keep_daily.is_some()
                        || r.keep_weekly.is_some()
                        || r.keep_monthly.is_some()
                        || r.keep_yearly.is_some()
                    {
                        // Tell the Activity panel this plan has moved from the byte-transfer
                        // phase to the retention finalize step (a `forget --prune` that can
                        // take 10s+). The panel keeps the active task visible and swaps its
                        // subtitle to "Applying retention rules…" (mirroring the manual-backup
                        // modal) instead of going blank. Emitted only when retention actually
                        // runs (plan succeeded + at least one keep flag set), so a plan with
                        // no retention dismisses immediately via backup-finished below.
                        let _ = app.emit("scheduler:retention-started", ());
                        if let Err(e) = apply_retention(&db, &master_key, &repo_locks, &plan.repo_id, &plan.tags, &plan.paths, r) {
                            log_retention_failure(app, &db, &plan.repo_id, Some(&plan.id), &e);
                        }
                    }
                }
            }

            // Dismiss the active task only once the plan is fully done (backup + retention),
            // then immediately hand off to the next plan's backup-started. Emitted here
            // (after retention) rather than right after execute_backup so the panel doesn't
            // blank out mid-plan — previously this fired before retention, hiding the
            // ~10-20s forget as a dead gap between two plans' backups.
            let _ = app.emit("scheduler:backup-finished", serde_json::json!({ "success": ok }));
        }

        let next = next_fire_time(&sched.cron_expr).ok();
        let _ = db.record_schedule_run(&sched.id, now, next);
        // record_schedule_run just advanced this schedule's next_run_at to the next
        // fire time. Emit schedules:changed so the Activity panel's Upcoming Tasks
        // section re-reads the now-future value — without this it keeps showing the
        // stale past timestamp ("just now") that made the schedule due in the first
        // place. (scheduler:backup-finished fires earlier, per-plan, before the
        // next_run_at update, so it can't carry the refreshed value.)
        let _ = app.emit("schedules:changed", ());
    }
}
