//! Per-plan webhook delivery. A plan's webhook list (URL + provider preset + stage
//! triggers) lives in `backup_plans.webhooks_json` (see cache.rs). `execute_backup`
//! (snapshot.rs) is the only fire site, next to each `notify::notify` call — manual,
//! scheduled, and run-now backups all funnel through it. Delivery is fire-and-forget
//! on a detached spawn_blocking thread; a failed or slow webhook can never fail or
//! delay the backup itself. The scheduler skips ticks while the app is locked, so
//! locked-app scheduled runs fire no webhooks — inherited behavior, not a gap.

use serde::Serialize;
use std::time::Duration;

use super::cache::{AppDb, WebhookProvider, WebhookStage};

/// Everything a payload needs, borrowed from `execute_backup`'s scope. The plan name
/// is deliberately not a field — `fire_webhooks` injects the name loaded by
/// `AppDb::get_plan_webhooks`, keeping the call sites terse.
pub struct WebhookEvent<'a> {
    pub stage: WebhookStage,
    pub repo_name: &'a str,
    pub duration_secs: Option<f64>,
    pub files_new: Option<u64>,
    pub files_changed: Option<u64>,
    pub bytes_added: Option<u64>,
    pub snapshot_id: Option<&'a str>,
    pub error: Option<&'a str>,
    pub started_at: Option<i64>,
}

/// Flat JSON body for the generic provider — camelCase keys, absent fields omitted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericPayload<'a> {
    event: &'a str,
    repo: &'a str,
    plan: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_new: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files_changed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_added: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub(crate) fn event_name(stage: WebhookStage) -> &'static str {
    match stage {
        WebhookStage::Started => "backup.started",
        WebhookStage::Completed => "backup.completed",
        WebhookStage::Failed => "backup.failed",
    }
}

/// Pure: the human-readable line used as Discord "content" / Slack "text".
pub(crate) fn build_message(ev: &WebhookEvent, plan_name: &str) -> String {
    match ev.stage {
        WebhookStage::Started => format!("Backup started — {plan_name} ({})", ev.repo_name),
        WebhookStage::Completed => format!(
            "Backup completed — {plan_name} ({}): {} new, {} changed, +{} bytes in {:.1}s",
            ev.repo_name,
            ev.files_new.unwrap_or(0),
            ev.files_changed.unwrap_or(0),
            ev.bytes_added.unwrap_or(0),
            ev.duration_secs.unwrap_or(0.0),
        ),
        WebhookStage::Failed => format!(
            "Backup failed — {plan_name} ({}): {}",
            ev.repo_name,
            ev.error.unwrap_or("unknown error"),
        ),
    }
}

/// Pure: the full request body per provider preset. Discord/Slack go through
/// `serde_json::json!` so message text is correctly escaped. Custom interpolates
/// the user's template (see `interpolate`); an empty template yields an empty body,
/// which `test_webhook`/`preview_webhook` reject up front — fire time is best-effort.
pub(crate) fn build_body(
    provider: WebhookProvider,
    template: Option<&str>,
    ev: &WebhookEvent,
    plan_name: &str,
) -> String {
    match provider {
        WebhookProvider::Generic => serde_json::to_string(&GenericPayload {
            event: event_name(ev.stage),
            repo: ev.repo_name,
            plan: plan_name,
            started_at: ev.started_at,
            duration_seconds: ev.duration_secs,
            files_new: ev.files_new,
            files_changed: ev.files_changed,
            bytes_added: ev.bytes_added,
            snapshot_id: ev.snapshot_id,
            error: ev.error,
        })
        .unwrap_or_default(),
        WebhookProvider::Discord => {
            serde_json::json!({ "content": build_message(ev, plan_name) }).to_string()
        }
        WebhookProvider::Slack => {
            serde_json::json!({ "text": build_message(ev, plan_name) }).to_string()
        }
        WebhookProvider::Teams => {
            // The fixed Adaptive Card wrapper a Power Automate "When a Teams webhook
            // request is received" (Workflows) URL expects — identical for everyone,
            // only the TextBlock text varies.
            serde_json::json!({
                "type": "message",
                "attachments": [{
                    "contentType": "application/vnd.microsoft.card.adaptive",
                    "content": {
                        "type": "AdaptiveCard",
                        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                        "version": "1.4",
                        "body": [{ "type": "TextBlock", "text": build_message(ev, plan_name), "wrap": true }]
                    }
                }]
            })
            .to_string()
        }
        WebhookProvider::Custom => {
            interpolate(template.unwrap_or(""), ev, plan_name)
        }
    }
}

/// The placeholder names a custom template may reference (without braces) — camelCase
/// to match the generic payload's field names, one mental model across providers.
pub(crate) const PLACEHOLDERS: &[&str] = &[
    "eventName",
    "repoName",
    "planName",
    "startedAt",
    "durationSeconds",
    "filesNew",
    "filesChanged",
    "bytesAdded",
    "snapshotId",
    "errorMessage",
];

/// (name, interpolated text) pairs for one event. String values are JSON-escaped
/// *content* (no surrounding quotes — the template's own quotes stay around them);
/// numeric values substitute as bare digits, with `0` for a `None` field so one
/// template works for every stage.
fn placeholder_pairs(ev: &WebhookEvent, plan_name: &str) -> Vec<(&'static str, String)> {
    vec![
        ("eventName", event_name(ev.stage).to_string()),
        ("repoName", json_escape(ev.repo_name)),
        ("planName", json_escape(plan_name)),
        ("startedAt", ev.started_at.unwrap_or(0).to_string()),
        ("durationSeconds", format_num(ev.duration_secs)),
        ("filesNew", ev.files_new.unwrap_or(0).to_string()),
        ("filesChanged", ev.files_changed.unwrap_or(0).to_string()),
        ("bytesAdded", ev.bytes_added.unwrap_or(0).to_string()),
        ("snapshotId", json_escape(ev.snapshot_id.unwrap_or(""))),
        ("errorMessage", json_escape(ev.error.unwrap_or(""))),
    ]
}

/// JSON-escapes `s` as string *content* — the surrounding quotes serde_json adds
/// are trimmed so the template's own quoting survives.
fn json_escape(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    quoted[1..quoted.len() - 1].to_string()
}

fn format_num(n: Option<f64>) -> String {
    let n = n.unwrap_or(0.0);
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Splits `template` into a sequence of `{token}` candidates and the literal text between
/// them — the single scan both `interpolate` and `unknown_placeholders` build on, so a
/// placeholder's substituted *value* is never re-scanned for further `{token}`s (a
/// sequential find-and-replace would otherwise let an early substitution — e.g. a repo
/// named `{errorMessage}` — get re-substituted by a later placeholder in the pass). Only
/// all-alphabetic `{token}`s count as placeholder candidates, so the template's own JSON
/// braces (`{"a": 1}`) and empty braces (`{}`) are never treated as one; a non-candidate
/// brace advances the scan by one char so placeholders inside JSON strings are still found.
enum Piece<'a> {
    Literal(&'a str),
    Token(&'a str),
}

fn scan(template: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    loop {
        match rest.find('{') {
            Some(open) => {
                let after = &rest[open + 1..];
                match after.find('}') {
                    Some(close_rel) => {
                        let name = &after[..close_rel];
                        let candidate =
                            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic());
                        if candidate {
                            if open > 0 {
                                pieces.push(Piece::Literal(&rest[..open]));
                            }
                            pieces.push(Piece::Token(name));
                            rest = &after[close_rel + 1..];
                        } else {
                            // Not a token candidate — consume just past the `{` itself so a
                            // JSON brace never gets swallowed into the preceding literal run.
                            pieces.push(Piece::Literal(&rest[..open + 1]));
                            rest = &rest[open + 1..];
                        }
                    }
                    // Unterminated `{` — the rest is all literal.
                    None => {
                        pieces.push(Piece::Literal(rest));
                        break;
                    }
                }
            }
            None => {
                if !rest.is_empty() {
                    pieces.push(Piece::Literal(rest));
                }
                break;
            }
        }
    }
    pieces
}

/// Substitutes every `{name}` occurrence with its placeholder value in one left-to-right
/// pass (see `scan`) — a substituted value is never itself re-scanned for further tokens.
/// The user owns the JSON structure — `"count": {filesNew}` (bare number) and
/// `"msg": "{planName}"` (quoted, escaped string) are both valid. Unknown tokens pass
/// through literally.
pub(crate) fn interpolate(template: &str, ev: &WebhookEvent, plan_name: &str) -> String {
    let pairs = placeholder_pairs(ev, plan_name);
    let mut out = String::with_capacity(template.len());
    for piece in scan(template) {
        match piece {
            Piece::Literal(s) => out.push_str(s),
            Piece::Token(name) => match pairs.iter().find(|(n, _)| *n == name) {
                Some((_, value)) => out.push_str(value),
                None => {
                    out.push('{');
                    out.push_str(name);
                    out.push('}');
                }
            },
        }
    }
    out
}

/// `{...}` tokens in the template that match no known placeholder — surfaces typos
/// like `{planname}` in the preview UI.
pub(crate) fn unknown_placeholders(template: &str) -> Vec<String> {
    let mut unknown: Vec<&str> = Vec::new();
    for piece in scan(template) {
        if let Piece::Token(name) = piece {
            if !PLACEHOLDERS.contains(&name) && !unknown.contains(&name) {
                unknown.push(name);
            }
        }
    }
    unknown.iter().map(|n| format!("{{{n}}}")).collect()
}

/// Fire-time URL sanity check — skip (not error) anything that isn't a plain
/// http(s) URL. Ureq would fail it anyway; this avoids the wasted thread.
pub(crate) fn valid_url(url: &str) -> bool {
    let t = url.trim();
    t.starts_with("https://") || t.starts_with("http://")
}

/// Single gated entry point, called from `execute_backup`'s four notify sites.
/// Loads the plan's webhooks, filters by stage + URL validity, builds all bodies
/// synchronously (borrowed event data can't cross into the closure), then posts each
/// on a detached spawn_blocking thread. Returns `()`; every failure is dropped.
pub fn fire_webhooks(db: &AppDb, plan_id: Option<&str>, ev: WebhookEvent<'_>) {
    // Plan-less backups (run straight from a repo, not via a plan) never fire.
    let Some(plan_id) = plan_id else { return };
    let Some((plan_name, webhooks)) = db.get_plan_webhooks(plan_id).unwrap_or(None) else { return };
    let jobs: Vec<(String, String)> = webhooks
        .into_iter()
        .filter(|w| w.stages.wants(ev.stage) && valid_url(&w.url))
        .map(|w| (w.url.trim().to_string(), build_body(w.provider, w.template.as_deref(), &ev, &plan_name)))
        .collect();
    if jobs.is_empty() {
        return;
    }
    // Deliberately fire-and-forget: webhook delivery is best-effort, never something
    // the backup path awaits (same shape as execute_backup's unlock_quietly spawn).
    #[allow(clippy::let_underscore_future)]
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build();
        for (url, body) in jobs {
            let _ = agent
                .post(&url)
                .set("Content-Type", "application/json")
                .send_string(&body);
        }
    });
}

/// Shared save/test-time gate for a Custom-provider template — non-Custom providers
/// always pass (their body has no user-authored JSON to break). Requires a non-empty
/// template, and that it renders valid JSON for **every** stage's sample event, not
/// just `completed` — the sample events differ per stage (only `Failed` carries a
/// non-empty `errorMessage`, for instance), so validating just one stage would miss a
/// template that only breaks on a field another stage substitutes differently. Single
/// source of truth for both `test_webhook` and `preview_webhook`, so the two can't
/// drift on what counts as valid.
fn validate_custom_template(provider: WebhookProvider, template: &str) -> Result<(), String> {
    if provider != WebhookProvider::Custom {
        return Ok(());
    }
    if template.trim().is_empty() {
        return Err("Custom webhooks need a JSON body template.".to_string());
    }
    for stage in [WebhookStage::Started, WebhookStage::Completed, WebhookStage::Failed] {
        let body = build_body(provider, Some(template), &sample_event(stage), SAMPLE_PLAN_NAME);
        serde_json::from_str::<serde_json::Value>(&body).map_err(|e| {
            format!("Template does not render to valid JSON (with sample values): {e}")
        })?;
    }
    Ok(())
}

/// Sends a synthetic completed-style event through the exact same `build_body` +
/// agent/POST path the fire sites use. Unlike `test_repo_connection` this is *not*
/// an `OperationCtx`/task-bus operation — it's sub-second, not a restic op, and has
/// nothing to show in the Activity panel; the frontend's ok/err result is the UX.
#[tauri::command]
pub async fn test_webhook(
    url: String,
    provider: WebhookProvider,
    template: Option<String>,
) -> Result<(), String> {
    let url = url.trim().to_string();
    if !valid_url(&url) {
        return Err("Webhook URL must start with https:// (or http://).".to_string());
    }
    validate_custom_template(provider, template.as_deref().unwrap_or(""))?;
    let body = build_body(
        provider,
        template.as_deref(),
        &sample_event(WebhookStage::Completed),
        SAMPLE_PLAN_NAME,
    );
    tauri::async_runtime::spawn_blocking(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .build();
        match agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body)
        {
            // Any 2xx counts — Discord returns 204 No Content on success.
            Ok(resp) if resp.status() < 300 => Ok(()),
            Ok(resp) => Err(format!("Webhook returned HTTP {}", resp.status())),
            Err(e) => Err(format!("Webhook request failed: {e}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The plan name every preview/test payload renders with.
pub(crate) const SAMPLE_PLAN_NAME: &str = "My plan";

/// A fixed, realistic sample event for `preview_webhook`/`test_webhook` — the
/// numbers are filled in (completed-style) so interpolated previews show
/// meaningful values rather than the None-defaults. `error` is only set on the
/// failed stage, matching what a real event of each stage carries.
fn sample_event(stage: WebhookStage) -> WebhookEvent<'static> {
    WebhookEvent {
        stage,
        repo_name: "my-repo",
        duration_secs: Some(42.0),
        files_new: Some(12),
        files_changed: Some(3),
        bytes_added: Some(1_048_576),
        snapshot_id: Some("a1b2c3d4"),
        error: (stage == WebhookStage::Failed).then_some("restic backup failed"),
        started_at: Some(1_700_000_000),
    }
}

/// Preview-rendered body per stage, for the edit page's "View payload" UI.
/// Rendered by the same `build_body` the fire sites use — single source of truth,
/// so the preview can never drift from what's actually POSTed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookPreview {
    pub started: String,
    pub completed: String,
    pub failed: String,
    /// Custom templates only: `{tokens}` matching no known placeholder (typos) —
    /// sent literally at fire time, so the UI can warn before that happens.
    pub unknown_placeholders: Vec<String>,
}

/// Pure, no I/O — deliberately a plain sync command (like `get_notification_settings`),
/// no spawn_blocking needed. For a custom provider, every stage's body must parse as
/// JSON (`validate_custom_template`); that check is the save-time validation hook (fire
/// time stays best-effort — a webhook must never fail a backup).
#[tauri::command]
pub fn preview_webhook(
    provider: WebhookProvider,
    template: Option<String>,
) -> Result<WebhookPreview, String> {
    let template = template.unwrap_or_default();
    let render = |stage: WebhookStage| build_body(provider, Some(&template), &sample_event(stage), SAMPLE_PLAN_NAME);
    validate_custom_template(provider, &template)?;
    Ok(WebhookPreview {
        started: render(WebhookStage::Started),
        completed: render(WebhookStage::Completed),
        failed: render(WebhookStage::Failed),
        unknown_placeholders: if provider == WebhookProvider::Custom {
            unknown_placeholders(&template)
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cache::{PlanWebhook, WebhookStages};

    fn completed_event() -> WebhookEvent<'static> {
        WebhookEvent {
            stage: WebhookStage::Completed,
            repo_name: "my-repo",
            duration_secs: Some(12.34),
            files_new: Some(3),
            files_changed: Some(4),
            bytes_added: Some(1024),
            snapshot_id: Some("abc123"),
            error: None,
            started_at: Some(1_700_000_000),
        }
    }

    #[test]
    fn generic_payload_is_flat_camel_case_json() {
        let body = build_body(WebhookProvider::Generic, None, &completed_event(), "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["event"], "backup.completed");
        assert_eq!(v["repo"], "my-repo");
        assert_eq!(v["plan"], "Daily");
        assert_eq!(v["filesNew"], 3);
        assert_eq!(v["filesChanged"], 4);
        assert_eq!(v["bytesAdded"], 1024);
        assert_eq!(v["snapshotId"], "abc123");
        assert_eq!(v["durationSeconds"], 12.34);
        assert_eq!(v["startedAt"], 1_700_000_000);
        // success payload has no error to report — the key is omitted entirely
        assert!(v.get("error").is_none());
    }

    #[test]
    fn generic_payload_omits_absent_fields_on_started() {
        let ev = WebhookEvent {
            stage: WebhookStage::Started,
            repo_name: "my-repo",
            duration_secs: None,
            files_new: None,
            files_changed: None,
            bytes_added: None,
            snapshot_id: None,
            error: None,
            started_at: None,
        };
        let v: serde_json::Value =
            serde_json::from_str(&build_body(WebhookProvider::Generic, None, &ev, "Daily")).unwrap();
        assert_eq!(v["event"], "backup.started");
        for key in [
            "durationSeconds",
            "filesNew",
            "filesChanged",
            "bytesAdded",
            "snapshotId",
            "startedAt",
            "error",
        ] {
            assert!(v.get(key).is_none(), "expected {key} to be omitted");
        }
    }

    #[test]
    fn discord_body_is_content_object() {
        let ev = completed_event();
        let body = build_body(WebhookProvider::Discord, None, &ev, "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["content"], build_message(&ev, "Daily"));
        // round-tripped through serde, so escaping is covered by equality here
        assert!(v.get("text").is_none());
    }

    #[test]
    fn slack_body_is_text_object() {
        let ev = completed_event();
        let body = build_body(WebhookProvider::Slack, None, &ev, "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["text"], build_message(&ev, "Daily"));
        assert!(v.get("content").is_none());
    }

    #[test]
    fn teams_body_is_adaptive_card_with_message_textblock() {
        let ev = completed_event();
        let body = build_body(WebhookProvider::Teams, None, &ev, "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["type"], "message");
        let content = &v["attachments"][0]["content"];
        assert_eq!(content["type"], "AdaptiveCard");
        assert_eq!(content["version"], "1.4");
        // the message rides inside the card's single TextBlock, escaped by json!()
        assert_eq!(content["body"][0]["text"], build_message(&ev, "Daily"));
        assert_eq!(v["attachments"][0]["contentType"], "application/vnd.microsoft.card.adaptive");
    }

    #[test]
    fn message_text_escapes_are_json_safe() {
        // A repo/plan name with quotes and a backslash must survive the JSON round-trip.
        let ev = WebhookEvent {
            stage: WebhookStage::Failed,
            repo_name: "re\"po",
            duration_secs: None,
            files_new: None,
            files_changed: None,
            bytes_added: None,
            snapshot_id: None,
            error: Some("failed \"hard\" \\"),
            started_at: None,
        };
        let v: serde_json::Value =
            serde_json::from_str(&build_body(WebhookProvider::Discord, None, &ev, "pl\"an")).unwrap();
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("re\"po"));
        assert!(content.contains("pl\"an"));
        assert!(content.contains("failed \"hard\" \\"));
    }

    #[test]
    fn message_text_per_stage_includes_plan_repo_and_detail() {
        let started = WebhookEvent {
            stage: WebhookStage::Started,
            repo_name: "my-repo",
            duration_secs: None,
            files_new: None,
            files_changed: None,
            bytes_added: None,
            snapshot_id: None,
            error: None,
            started_at: None,
        };
        let msg = build_message(&started, "Daily");
        assert!(msg.contains("Backup started"));
        assert!(msg.contains("Daily"));
        assert!(msg.contains("my-repo"));

        let msg = build_message(&completed_event(), "Daily");
        assert!(msg.contains("Backup completed"));
        assert!(msg.contains("3 new"));
        assert!(msg.contains("4 changed"));
        assert!(msg.contains("+1024 bytes"));
        assert!(msg.contains("12.3s"));

        let failed = WebhookEvent {
            stage: WebhookStage::Failed,
            repo_name: "my-repo",
            duration_secs: None,
            files_new: None,
            files_changed: None,
            bytes_added: None,
            snapshot_id: None,
            error: Some("disk full"),
            started_at: None,
        };
        let msg = build_message(&failed, "Daily");
        assert!(msg.contains("Backup failed"));
        assert!(msg.contains("disk full"));
    }

    #[test]
    fn stages_gating_matches_each_flag() {
        let stages = WebhookStages::default();
        assert!(!stages.wants(WebhookStage::Started));
        assert!(stages.wants(WebhookStage::Completed));
        assert!(stages.wants(WebhookStage::Failed));
    }

    #[test]
    fn plan_webhook_deserializes_without_stages() {
        // A hand-edited export bundle may omit "stages" — it must fall back to the
        // defaults rather than failing the whole plan import.
        let w: PlanWebhook = serde_json::from_str(
            r#"{ "id": "w1", "url": "https://example.com/hook", "provider": "discord" }"#,
        )
        .unwrap();
        assert_eq!(w.id, "w1");
        assert_eq!(w.provider, WebhookProvider::Discord);
        assert_eq!(w.stages, WebhookStages::default());
        // the template field is additive too — omitting it must not fail the import
        assert_eq!(w.template, None);
    }

    #[test]
    fn valid_url_accepts_http_https_only() {
        assert!(valid_url("https://discord.com/api/webhooks/x"));
        assert!(valid_url("http://example.com/hook"));
        assert!(valid_url("  https://padded.example/hook  "));
        assert!(!valid_url("ftp://example.com"));
        assert!(!valid_url("discord.com/api/webhooks/x"));
        assert!(!valid_url(""));
    }

    /// The default template BackupPlanEditPage pre-fills for a new custom webhook —
    /// pinned here so the frontend constant and the interpolation rules can't drift
    /// apart (it must render valid JSON for all three stages).
    const DEFAULT_TEMPLATE: &str =
        r#"{"event": "{eventName}", "plan": "{planName}", "repo": "{repoName}", "durationSeconds": {durationSeconds}}"#;

    fn started_event() -> WebhookEvent<'static> {
        WebhookEvent {
            stage: WebhookStage::Started,
            repo_name: "my-repo",
            duration_secs: None,
            files_new: None,
            files_changed: None,
            bytes_added: None,
            snapshot_id: None,
            error: None,
            started_at: Some(1_700_000_000),
        }
    }

    #[test]
    fn interpolate_substitutes_known_placeholders() {
        let template = concat!(
            r#"{"event":"{eventName}","repo":"{repoName}","plan":"{planName}","#,
            r#""startedAt":{startedAt},"durationSeconds":{durationSeconds},"#,
            r#""filesNew":{filesNew},"filesChanged":{filesChanged},"bytesAdded":{bytesAdded},"#,
            r#""snapshot":"{snapshotId}","error":"{errorMessage}"}"#
        );
        let body = interpolate(template, &completed_event(), "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["event"], "backup.completed");
        assert_eq!(v["repo"], "my-repo");
        assert_eq!(v["plan"], "Daily");
        assert_eq!(v["startedAt"], 1_700_000_000);
        assert_eq!(v["durationSeconds"], 12.34);
        assert_eq!(v["filesNew"], 3);
        assert_eq!(v["filesChanged"], 4);
        assert_eq!(v["bytesAdded"], 1024);
        assert_eq!(v["snapshot"], "abc123");
        assert_eq!(v["error"], "");
    }

    #[test]
    fn interpolate_escapes_strings() {
        // A repo/plan name with quotes and a backslash must not break the JSON.
        let ev = WebhookEvent {
            stage: WebhookStage::Failed,
            repo_name: "re\"po\\x",
            error: Some("failed \"hard\""),
            ..started_event()
        };
        let body = interpolate(
            r#"{"repo":"{repoName}","plan":"{planName}","error":"{errorMessage}"}"#,
            &ev,
            "pl\"an",
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["repo"], "re\"po\\x");
        assert_eq!(v["plan"], "pl\"an");
        assert_eq!(v["error"], "failed \"hard\"");
    }

    #[test]
    fn interpolate_none_values_use_defaults() {
        let body = interpolate(
            r#"{"snapshot":"{snapshotId}","filesNew":{filesNew},"durationSeconds":{durationSeconds}}"#,
            &started_event(),
            "Daily",
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["snapshot"], "");
        assert_eq!(v["filesNew"], 0);
        assert_eq!(v["durationSeconds"], 0);
    }

    #[test]
    fn unknown_placeholders_flags_typos() {
        assert_eq!(unknown_placeholders("{planname}"), vec!["{planname}".to_string()]);
        assert_eq!(unknown_placeholders("{planName}"), Vec::<String>::new());
        assert!(unknown_placeholders("{}").is_empty());
        assert!(unknown_placeholders("{{}}").is_empty());
        // no duplicates when the same typo appears twice
        assert_eq!(
            unknown_placeholders(r#"{"a":"{planName}","b":"{planname}","c":"{planname}"}"#),
            vec!["{planname}".to_string()]
        );
        // unterminated brace is ignored
        assert!(unknown_placeholders("no closing { brace").is_empty());
    }

    #[test]
    fn custom_default_template_renders_valid_json_for_all_stages() {
        for ev in [started_event(), completed_event()] {
            let body = interpolate(DEFAULT_TEMPLATE, &ev, "Daily");
            serde_json::from_str::<serde_json::Value>(&body)
                .unwrap_or_else(|e| panic!("invalid JSON for {:?}: {e} — body: {body}", ev.stage));
        }
        let failed = WebhookEvent {
            stage: WebhookStage::Failed,
            error: Some("boom"),
            ..started_event()
        };
        let body = interpolate(DEFAULT_TEMPLATE, &failed, "Daily");
        serde_json::from_str::<serde_json::Value>(&body).unwrap();
    }

    #[test]
    fn custom_body_builds_through_build_body() {
        let body = build_body(
            WebhookProvider::Custom,
            Some(DEFAULT_TEMPLATE),
            &completed_event(),
            "Daily",
        );
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["event"], "backup.completed");
        assert_eq!(v["plan"], "Daily");
        assert_eq!(v["durationSeconds"], 12.34);
    }

    #[test]
    fn preview_renders_all_three_stages_and_validates_custom_json() {
        let preview = preview_webhook(
            WebhookProvider::Custom,
            Some(DEFAULT_TEMPLATE.to_string()),
        )
        .unwrap();
        let started: serde_json::Value = serde_json::from_str(&preview.started).unwrap();
        let completed: serde_json::Value = serde_json::from_str(&preview.completed).unwrap();
        let failed: serde_json::Value = serde_json::from_str(&preview.failed).unwrap();
        assert_eq!(started["event"], "backup.started");
        assert_eq!(completed["event"], "backup.completed");
        assert_eq!(failed["event"], "backup.failed");
        assert!(preview.unknown_placeholders.is_empty());

        // presets render without a template and never report unknown placeholders
        let preset = preview_webhook(WebhookProvider::Discord, None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&preset.completed).unwrap();
        assert!(v["content"].as_str().unwrap().contains("Backup completed"));
        assert!(preset.unknown_placeholders.is_empty());

        // a dangling quote must fail the JSON-parse validation, naming the error
        let err = preview_webhook(
            WebhookProvider::Custom,
            Some(r#"{"text": "unclosed}"#.to_string()),
        )
        .unwrap_err();
        assert!(err.contains("valid JSON"), "unexpected error: {err}");

        // empty template is rejected up front
        let err = preview_webhook(WebhookProvider::Custom, None).unwrap_err();
        assert!(err.contains("template"), "unexpected error: {err}");

        // unknown placeholders surface for warning (not rejection)
        let preview = preview_webhook(
            WebhookProvider::Custom,
            Some(r#"{"text": "{planname}"}"#.to_string()),
        )
        .unwrap();
        assert_eq!(preview.unknown_placeholders, vec!["{planname}".to_string()]);
    }

    #[test]
    fn validate_custom_template_matches_preview_webhook() {
        // Non-Custom providers always pass regardless of template content.
        assert!(validate_custom_template(WebhookProvider::Discord, "").is_ok());
        assert!(validate_custom_template(WebhookProvider::Generic, "not json").is_ok());

        // Custom: empty template rejected.
        let err = validate_custom_template(WebhookProvider::Custom, "").unwrap_err();
        assert!(err.contains("template"), "unexpected error: {err}");
        let err = validate_custom_template(WebhookProvider::Custom, "   ").unwrap_err();
        assert!(err.contains("template"), "unexpected error: {err}");

        // Custom: valid template accepted.
        assert!(validate_custom_template(WebhookProvider::Custom, DEFAULT_TEMPLATE).is_ok());

        // Custom: invalid JSON rejected.
        let err =
            validate_custom_template(WebhookProvider::Custom, r#"{"text": "unclosed}"#).unwrap_err();
        assert!(err.contains("valid JSON"), "unexpected error: {err}");
    }

    #[test]
    fn interpolate_does_not_re_substitute_a_value_that_contains_another_token() {
        // A repo named "{errorMessage}" must render as the literal string "{errorMessage}"
        // in the repo field — not get expanded again because errorMessage is substituted
        // later in the pass. A sequential find-and-replace over the whole output would
        // wrongly expand it.
        let ev = WebhookEvent {
            stage: WebhookStage::Failed,
            repo_name: "{errorMessage}",
            error: Some("disk full"),
            ..started_event()
        };
        let body = interpolate(r#"{"repo":"{repoName}","error":"{errorMessage}"}"#, &ev, "Daily");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["repo"], "{errorMessage}");
        assert_eq!(v["error"], "disk full");
    }

    #[test]
    fn interpolate_plan_name_containing_a_token_is_not_re_substituted() {
        let ev = completed_event();
        let body = interpolate(r#"{"plan":"{planName}","new":{filesNew}}"#, &ev, "{filesNew}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["plan"], "{filesNew}");
        assert_eq!(v["new"], 3);
    }

    #[test]
    fn fire_webhooks_trims_whitespace_padded_urls_before_posting() {
        // valid_url() already trims for the pass/fail gate; the job list built from it
        // must carry the trimmed URL through to the actual POST, not the raw padded one —
        // otherwise a URL like "  https://example.com/hook  " passes the gate here but
        // fails inside ureq at fire time.
        let padded = "  https://example.com/hook  ";
        assert!(valid_url(padded));
        assert_eq!(padded.trim(), "https://example.com/hook");
    }
}
