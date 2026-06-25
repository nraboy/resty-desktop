use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::Manager;

use crate::commands::cache::{AppDb, BackupHandle, MasterKey};
use crate::commands::schedule::next_fire_time;
use crate::commands::snapshot::{apply_retention, execute_backup};

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
            let ok = execute_backup(
                app,
                &db,
                &master_key,
                &*backup_handle,
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
                        let _ = apply_retention(&db, &master_key, &plan.repo_id, &plan.tags, &plan.paths, r);
                    }
                }
            }
        }

        let next = next_fire_time(&sched.cron_expr).ok();
        let _ = db.record_schedule_run(&sched.id, now, next);
    }
}
