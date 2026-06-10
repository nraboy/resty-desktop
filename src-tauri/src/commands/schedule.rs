use cron::Schedule as CronSchedule;
use chrono::Local;
use std::str::FromStr;
use tauri::State;

use super::cache::{AppDb, MasterKey, Schedule};
use super::snapshot::execute_backup;

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
pub fn save_schedule(db: State<'_, AppDb>, schedule: Schedule) -> Result<(), String> {
    // Validate cron and compute next_run_at
    let next_run_at = next_fire_time(&schedule.cron_expr)
        .map(Some)
        .map_err(|e| format!("Invalid cron expression: {e}"))?;
    let s = Schedule { next_run_at, ..schedule };
    db.save_schedule(&s)
}

#[tauri::command]
pub fn remove_schedule(db: State<'_, AppDb>, schedule_id: String) -> Result<(), String> {
    db.remove_schedule(&schedule_id)
}

#[tauri::command]
pub fn toggle_schedule(db: State<'_, AppDb>, schedule_id: String, enabled: bool) -> Result<(), String> {
    db.set_schedule_enabled(&schedule_id, enabled)
}

#[tauri::command]
pub async fn run_schedule_now(
    app: tauri::AppHandle,
    db: State<'_, AppDb>,
    master_key: State<'_, MasterKey>,
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
        if let Err(e) = execute_backup(
            &app, &db, &master_key,
            &plan.repo_id, Some(&plan.id),
            plan.paths, plan.tags, plan.excludes,
        )
        .await
        {
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
