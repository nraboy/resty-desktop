use cron::Schedule as CronSchedule;
use chrono::Local;
use std::str::FromStr;
use tauri::{Emitter, State};

use super::cache::{AppDb, BackupHandle, MasterKey, Schedule};
use super::repo_locks::RepoLocks;
use super::snapshot::{apply_retention, execute_backup, log_retention_failure};

// ── cron helpers (pub(crate) so scheduler.rs can reuse) ───────────────────

fn to_7field(expr: &str) -> String {
    format!("0 {} *", expr.trim())
}

pub(crate) fn next_fire_time(expr: &str) -> Result<i64, String> {
    let full = to_7field(expr);
    let sched = CronSchedule::from_str(&full).map_err(|e| e.to_string())?;
    sched
        .upcoming(Local)
        .take(1)
        .next()
        .map(|dt| dt.timestamp())
        .ok_or_else(|| "No upcoming fire times".to_string())
}

pub(crate) fn describe_cron(expr: &str) -> String {
    let parts: Vec<&str> = expr.trim().split_whitespace().collect();
    if parts.len() != 5 {
        return expr.to_string();
    }
    let (min, hour, dom, _month, dow) = (parts[0], parts[1], parts[2], parts[3], parts[4]);

    let time_str = match (min.parse::<u32>(), hour.parse::<u32>()) {
        (Ok(m), Ok(h)) => format!("{:02}:{:02}", h, m),
        _ => return expr.to_string(),
    };

    if dow != "*" && dom == "*" {
        let day_name = match dow {
            "0" | "7" => "Sunday",
            "1" => "Monday",
            "2" => "Tuesday",
            "3" => "Wednesday",
            "4" => "Thursday",
            "5" => "Friday",
            "6" => "Saturday",
            _ => dow,
        };
        format!("Every {} at {}", day_name, time_str)
    } else if dom != "*" && dow == "*" {
        format!("Monthly on day {} at {}", dom, time_str)
    } else {
        format!("Daily at {}", time_str)
    }
}

// ── commands ──────────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_schedules(db: State<'_, AppDb>) -> Result<Vec<Schedule>, String> {
    db.list_schedules()
}

#[tauri::command]
pub fn save_schedule(app: tauri::AppHandle, db: State<'_, AppDb>, schedule: Schedule) -> Result<(), String> {
    // Validate cron and compute next_run_at
    let next_run_at = next_fire_time(&schedule.cron_expr)
        .map(Some)
        .map_err(|e| format!("Invalid cron expression: {e}"))?;
    let s = Schedule { next_run_at, ..schedule };
    db.save_schedule(&s)?;
    let _ = app.emit("schedules:changed", ());
    Ok(())
}

#[tauri::command]
pub fn remove_schedule(app: tauri::AppHandle, db: State<'_, AppDb>, schedule_id: String) -> Result<(), String> {
    db.remove_schedule(&schedule_id)?;
    let _ = app.emit("schedules:changed", ());
    Ok(())
}

#[tauri::command]
pub fn toggle_schedule(app: tauri::AppHandle, db: State<'_, AppDb>, schedule_id: String, enabled: bool) -> Result<(), String> {
    db.set_schedule_enabled(&schedule_id, enabled)?;
    let _ = app.emit("schedules:changed", ());
    Ok(())
}

#[tauri::command]
pub async fn run_schedule_now(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
    backup_handle: State<'_, BackupHandle>,
    repo_locks: State<'_, RepoLocks>,
    schedule_id: String,
) -> Result<(), String> {
    let schedules = db.list_schedules()?;
    let sched = schedules
        .into_iter()
        .find(|s| s.id == schedule_id)
        .ok_or_else(|| "Schedule not found".to_string())?;

    let plans = db.get_plans_for_ids(&sched.plan_ids)?;
    let mut errors: Vec<String> = Vec::new();
    for plan in plans {
        let backup_ok = execute_backup(
            &app, &db, &master_key, &backup_handle, &repo_locks,
            &plan.repo_id, Some(plan.id.as_str()),
            plan.paths.clone(), plan.tags.clone(), plan.excludes,
            plan.limit_upload, plan.limit_download,
        )
        .await;

        if backup_ok.is_ok() {
            if let Some(r) = &plan.retention {
                if r.keep_last.is_some()
                    || r.keep_daily.is_some()
                    || r.keep_weekly.is_some()
                    || r.keep_monthly.is_some()
                    || r.keep_yearly.is_some()
                {
                    if let Err(e) = apply_retention(&db, &master_key, &repo_locks, &plan.repo_id, &plan.tags, &plan.paths, r) {
                        log_retention_failure(&app, &db, &plan.repo_id, Some(&plan.id), &e);
                    }
                }
            }
        } else if let Err(e) = backup_ok {
            errors.push(format!("{}: {}", plan.name, e));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[tauri::command]
pub fn describe_cron_expr(cron_expr: String) -> String {
    describe_cron(&cron_expr)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_next_fire_time_daily() {
        // Daily at midnight - 5-field cron becomes 7-field
        let result = next_fire_time("0 0 * * *");
        assert!(result.is_ok());
        let ts = result.unwrap();
        // Should be in the future and within next day
        let now = chrono::Local::now().timestamp();
        assert!(ts > now);
        assert!(ts < now + 86400 + 1);
    }

    #[test]
    fn test_next_fire_time_hourly() {
        // Every hour at minute 0
        let result = next_fire_time("0 * * * *");
        assert!(result.is_ok());
        let ts = result.unwrap();
        let now = chrono::Local::now().timestamp();
        assert!(ts > now);
        assert!(ts < now + 3600 + 1);
    }

    #[test]
    fn test_next_fire_time_weekly() {
        // Weekly on Monday at 2:30 AM
        let result = next_fire_time("30 2 * * 1");
        assert!(result.is_ok(), "weekly schedule should parse correctly");
        let ts = result.unwrap();
        let now = chrono::Local::now().timestamp();
        assert!(ts > now);
        // Should be within next week
        assert!(ts < now + 7 * 86400 + 1);
    }

    #[test]
    fn test_next_fire_time_invalid() {
        // Invalid cron expressions
        assert!(next_fire_time("invalid").is_err());
        assert!(next_fire_time("60 * * * *").is_err());
        assert!(next_fire_time("* 25 * * *").is_err());
    }

    #[test]
    fn test_describe_cron_daily() {
        assert_eq!(describe_cron("0 0 * * *"), "Daily at 00:00");
        assert_eq!(describe_cron("30 12 * * *"), "Daily at 12:30");
        assert_eq!(describe_cron("15 3 * * *"), "Daily at 03:15");
    }

    #[test]
    fn test_describe_cron_weekly() {
        assert_eq!(describe_cron("0 2 * * 0"), "Every Sunday at 02:00");
        assert_eq!(describe_cron("30 8 * * 1"), "Every Monday at 08:30");
        assert_eq!(describe_cron("45 16 * * 5"), "Every Friday at 16:45");
        assert_eq!(describe_cron("0 12 * * 7"), "Every Sunday at 12:00");
    }

    #[test]
    fn test_describe_cron_monthly() {
        assert_eq!(describe_cron("0 0 1 * *"), "Monthly on day 1 at 00:00");
        assert_eq!(describe_cron("30 12 15 * *"), "Monthly on day 15 at 12:30");
        assert_eq!(describe_cron("0 3 20 * *"), "Monthly on day 20 at 03:00");
    }

    #[test]
    fn test_describe_cron_invalid_format() {
        // Too few parts
        assert_eq!(describe_cron("0 0 * *"), "0 0 * *");
        // Too many parts
        assert_eq!(describe_cron("0 0 * * * *"), "0 0 * * * *");
        // Invalid time (non-numeric)
        assert_eq!(describe_cron("abc * * * *"), "abc * * * *");
    }
}
