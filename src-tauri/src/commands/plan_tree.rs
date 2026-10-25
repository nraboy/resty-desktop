//! Commands behind the plan editor's file-tree preview: list a folder, classify paths against a
//! plan's rules, and scan a subtree for "is anything in here excluded?". The decisions
//! themselves come from [`super::plan_rules`] (restic's own matching); this module only feeds
//! it the plan's rules, reads the disk, and streams scan results to the UI.
//!
//! Nothing here persists anything: listings and verdicts live in the frontend's memory only
//! (see docs/backup-plan-tree.md). Scans read directory entries, never file contents.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::cache::ExcludeFile;
use super::plan_edit::{apply_edit, export_rules, EditAction, EditOutcome, ExportOut};
use super::plan_rules::{
    clean_path, strip_source, Engine, FsProbe, Marker, Pattern, PlanRules, Probe, Reason, RuleInput,
};

// ── building the engine from a plan ─────────────────────────────────────────

/// The parts of a backup plan that decide what is included, as the editor holds them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeRulesInput {
    pub sources: Vec<String>,
    /// `--exclude` patterns, in order (blank lines and `#` comments are dropped here, exactly as
    /// `build_exclude_args` does at backup time).
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub exclude_if_present: Vec<String>,
    #[serde(default)]
    pub exclude_caches: bool,
    #[serde(default)]
    pub exclude_files: Vec<ExcludeFile>,
}

/// Where a rule came from, so a verdict can say "excluded by line 12 of ~/.ignore".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuleOrigin {
    pub id: u32,
    pub pattern: String,
    /// `None` for a pattern typed into the plan; the file's path for an exclude-file line.
    pub file: Option<String>,
    pub line: Option<u32>,
    /// From an `--iexclude-file`: matched case-insensitively, in a list a plain `!` can't cancel.
    pub ignore_case: bool,
}

/// Something in the plan restic would refuse to run with, or a file we couldn't read.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuleProblem {
    pub what: String,
    pub error: String,
}

pub(crate) struct BuiltRules {
    pub rules: PlanRules,
    pub origins: Vec<RuleOrigin>,
    pub problems: Vec<RuleProblem>,
}

fn is_shell_special(b: u8) -> bool {
    matches!(b, b'*' | b'#' | b'$' | b'@' | b'!' | b'?' | b'-' | b'0'..=b'9')
}

/// Go's `os.Expand` with restic's mapping (`$$` → `$`, anything else from the environment) —
/// the expansion restic applies to every line of an exclude file (not to `--exclude` flags).
pub(crate) fn expand_env(s: &str, getenv: &dyn Fn(&str) -> String) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut j = 0;
    while j < bytes.len() {
        if bytes[j] == b'$' && j + 1 < bytes.len() {
            out.push_str(&s[i..j]);
            let rest = &bytes[j + 1..];
            // getShellName
            let (name, w): (&str, usize) = if rest[0] == b'{' {
                if rest.len() > 2 && is_shell_special(rest[1]) && rest[2] == b'}' {
                    (&s[j + 2..j + 3], 3)
                } else if let Some(close) = rest.iter().skip(1).position(|&b| b == b'}') {
                    let close = close + 1;
                    if close == 1 {
                        ("", 2) // "${}": bad syntax, eaten
                    } else {
                        (&s[j + 2..j + 1 + close], close + 1)
                    }
                } else {
                    ("", 1) // "${": bad syntax, eaten
                }
            } else if is_shell_special(rest[0]) {
                (&s[j + 1..j + 2], 1)
            } else {
                let n = rest.iter().take_while(|b| b.is_ascii_alphanumeric() || **b == b'_').count();
                (&s[j + 1..j + 1 + n], n)
            };
            if name.is_empty() && w > 0 {
                // invalid syntax: eat the characters
            } else if name.is_empty() || name == "$" {
                // A lone '$' stays literal, and '$$' is restic's escape for it.
                out.push('$');
            } else {
                out.push_str(&getenv(name));
            }
            j += w;
            i = j + 1;
        }
        j += 1;
    }
    if i < s.len() {
        out.push_str(&s[i.min(s.len())..]);
    }
    out
}

/// Assembles the engine's input from a plan, in the order the patterns reach restic: the
/// `--exclude` flags first, then each exclude file's lines (case-sensitive files extend that
/// list, `--iexclude-file`s form the separate case-insensitive one). Invalid patterns and
/// unreadable files are reported in `problems`, not fatal — the preview should still show.
pub(crate) fn build_rules(
    input: &TreeRulesInput,
    read_file: &dyn Fn(&str) -> std::io::Result<String>,
    getenv: &dyn Fn(&str) -> String,
) -> BuiltRules {
    let mut rules: Vec<RuleInput> = Vec::new();
    let mut origins: Vec<RuleOrigin> = Vec::new();
    let mut problems: Vec<RuleProblem> = Vec::new();

    let mut push = |pattern: String, ignore_case: bool, file: Option<String>, line: Option<u32>,
                    problems: &mut Vec<RuleProblem>| {
        let probe = if ignore_case { pattern.to_lowercase() } else { pattern.clone() };
        if Pattern::parse(&probe).is_err() {
            problems.push(RuleProblem {
                what: match (&file, line) {
                    (Some(f), Some(l)) => format!("{f}:{l}"),
                    _ => pattern.clone(),
                },
                error: format!("invalid pattern: {pattern}"),
            });
            return;
        }
        let id = rules.len() as u32 + 1;
        rules.push(RuleInput { id, pattern: pattern.clone(), ignore_case });
        origins.push(RuleOrigin { id, pattern, file, line, ignore_case });
    };

    for raw in &input.excludes {
        let t = raw.trim();
        if !t.is_empty() && !t.starts_with('#') {
            push(t.to_string(), false, None, None, &mut problems);
        }
    }
    for f in &input.exclude_files {
        let path = f.path.trim();
        if path.is_empty() {
            continue;
        }
        let text = match read_file(path) {
            Ok(t) => t,
            Err(e) => {
                problems.push(RuleProblem { what: path.to_string(), error: format!("can't read exclude file: {e}") });
                continue;
            }
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            push(expand_env(line, getenv), f.ignore_case, Some(path.to_string()), Some(n as u32 + 1), &mut problems);
        }
    }

    let mut markers = Vec::new();
    for raw in &input.exclude_if_present {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        match Marker::parse(t) {
            Ok(_) => markers.push(t.to_string()),
            Err(e) => problems.push(RuleProblem { what: t.to_string(), error: e }),
        }
    }

    BuiltRules {
        rules: PlanRules {
            sources: input.sources.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string).collect(),
            rules,
            markers,
            exclude_caches: input.exclude_caches,
        },
        origins,
        problems,
    }
}

fn real_build(input: &TreeRulesInput) -> Result<(Engine, Vec<RuleOrigin>, Vec<RuleProblem>), String> {
    let built = build_rules(
        input,
        &|p| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()),
        &|name| std::env::var(name).unwrap_or_default(),
    );
    let engine = Engine::new(&built.rules)?;
    Ok((engine, built.origins, built.problems))
}

// ── listing folders ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TreeEntry {
    pub name: String,
    /// `dir`, `file`, `symlink` (never followed — restic stores the link itself) or `other`.
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TreeListing {
    pub entries: Vec<TreeEntry>,
    /// Entries in the folder (before the cap).
    pub total: usize,
    pub truncated: bool,
    /// Set when the folder couldn't be read (permission denied, gone, …).
    pub error: Option<String>,
}

/// Lists a folder: folders first, then the rest, each group case-insensitively by name, capped
/// at `limit`. Reads names and entry types only — no `stat`, no file contents.
pub(crate) fn list_dir(path: &str, limit: usize) -> TreeListing {
    let rd = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => return TreeListing { entries: vec![], total: 0, truncated: false, error: Some(e.to_string()) },
    };
    let mut entries: Vec<TreeEntry> = Vec::new();
    for ent in rd.flatten() {
        // A name that isn't valid UTF-8 can't round-trip through the UI as a path.
        let Some(name) = ent.file_name().to_str().map(str::to_string) else { continue };
        let kind = match ent.file_type() {
            Ok(t) if t.is_symlink() => "symlink",
            Ok(t) if t.is_dir() => "dir",
            Ok(t) if t.is_file() => "file",
            _ => "other",
        };
        entries.push(TreeEntry { name, kind });
    }
    entries.sort_by(|a, b| {
        (a.kind != "dir")
            .cmp(&(b.kind != "dir"))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let total = entries.len();
    let truncated = total > limit;
    entries.truncate(limit);
    TreeListing { entries, total, truncated, error: None }
}

/// What each path is: `dir`, `file`, `symlink`, `other`, or `missing`. With `follow`, a symlink
/// reports its target's kind (how a symlink *source* is treated); otherwise it is `symlink`.
pub(crate) fn kinds(paths: &[String], follow: bool) -> Vec<&'static str> {
    paths
        .iter()
        .map(|p| {
            let md = if follow { std::fs::metadata(p) } else { std::fs::symlink_metadata(p) };
            match md {
                Err(_) => "missing",
                Ok(m) if m.file_type().is_symlink() => "symlink",
                Ok(m) if m.is_dir() => "dir",
                Ok(m) if m.is_file() => "file",
                Ok(_) => "other",
            }
        })
        .collect()
}

// ── classifying paths ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReasonOut {
    /// `pattern`, `marker` or `outsideSources`.
    pub kind: &'static str,
    pub rule_id: Option<u32>,
    pub spec: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeVerdict {
    pub path: String,
    pub included: bool,
    pub reason: Option<ReasonOut>,
    /// The rejected ancestor, when the reason applies to one above this path.
    pub at: Option<String>,
    pub reincluded_by: Option<u32>,
    /// Not backed up itself, but a source lies somewhere below it (an ancestor of a source).
    pub contains_sources: bool,
}

pub(crate) fn classify_paths(engine: &Engine, probe: &dyn Probe, paths: &[String]) -> Vec<NodeVerdict> {
    paths
        .iter()
        .map(|p| {
            let path = clean_path(p);
            let v = engine.classify(&path, probe);
            let contains_sources = !v.included
                && engine
                    .sources()
                    .iter()
                    .any(|s| matches!(strip_source(s, &path), Some(rest) if !rest.is_empty()));
            NodeVerdict {
                path: p.clone(),
                included: v.included,
                reason: v.reason.map(|r| match r {
                    Reason::Pattern { rule_id } => ReasonOut { kind: "pattern", rule_id: Some(rule_id), spec: None },
                    Reason::Marker { spec } => ReasonOut { kind: "marker", rule_id: None, spec: Some(spec) },
                    Reason::OutsideSources => ReasonOut { kind: "outsideSources", rule_id: None, spec: None },
                }),
                at: v.at,
                reincluded_by: v.reincluded_by,
                contains_sources,
            }
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub nodes: Vec<NodeVerdict>,
    pub origins: Vec<RuleOrigin>,
    pub problems: Vec<RuleProblem>,
}

// ── scanning a subtree ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanLimits {
    pub budget: Duration,
    pub max_entries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScanOutcome {
    /// `partial` when something below is excluded, else `included`.
    pub partial: bool,
    /// The verdict is definitive: an exclusion was found, or everything was looked at.
    pub complete: bool,
    pub first_excluded: Option<String>,
    /// Some folder couldn't be read, so "nothing excluded" can't be asserted.
    pub unreadable: bool,
    pub scanned: u64,
}

/// Looks for the first excluded item below `dir` (itself assumed included), breadth first so
/// shallow exclusions (`node_modules`, `.git`) are found fast, never entering a rejected
/// directory or a symlink — exactly as restic walks. Stops early on the first exclusion, when
/// the budget runs out (`complete: false`), or when cancelled.
pub(crate) fn scan_dir(
    engine: &Engine,
    probe: &dyn Probe,
    dir: &str,
    limits: ScanLimits,
    cancel: &AtomicBool,
) -> ScanOutcome {
    let mut out = ScanOutcome { partial: false, complete: false, first_excluded: None, unreadable: false, scanned: 0 };
    if !engine.has_rules() {
        out.complete = true; // nothing can be excluded below a source
        return out;
    }
    let start = Instant::now();
    let mut queue: VecDeque<String> = VecDeque::from([dir.to_string()]);
    while let Some(current) = queue.pop_front() {
        let rd = match std::fs::read_dir(&current) {
            Ok(rd) => rd,
            Err(_) => {
                out.unreadable = true;
                continue;
            }
        };
        for ent in rd.flatten() {
            out.scanned += 1;
            if out.scanned & 0xFF == 0
                && (cancel.load(Ordering::Relaxed) || start.elapsed() > limits.budget || out.scanned > limits.max_entries)
            {
                return out; // gave up: complete stays false
            }
            let Some(name) = ent.file_name().to_str().map(str::to_string) else {
                out.unreadable = true;
                continue;
            };
            let path = if current == "/" { format!("/{name}") } else { format!("{current}/{name}") };
            if engine.judge_node(&path, probe).is_some() {
                out.partial = true;
                out.complete = true;
                out.first_excluded = Some(path);
                return out;
            }
            if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                queue.push_back(path);
            }
        }
    }
    out.complete = !out.unreadable;
    out
}

// ── previewing a pattern ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MatchPreview {
    /// Items that are backed up now and would be left out by the pattern (a folder counts once,
    /// not its contents).
    pub count: u64,
    pub sample: Vec<String>,
    /// The count is a lower bound: the scan stopped at its budget or its cap.
    pub capped: bool,
}

const MATCH_CAP: u64 = 5000;
const MATCH_SAMPLE: usize = 5;

/// How many currently-included items `with` (the rules plus a new pattern) would reject that
/// `base` doesn't: a breadth-first walk of every source folder that never enters an item either
/// engine rejects, like restic.
pub(crate) fn count_matches(
    base: &Engine,
    with: &Engine,
    probe: &dyn Probe,
    sources: &[String],
    limits: ScanLimits,
    cancel: &AtomicBool,
) -> MatchPreview {
    let mut out = MatchPreview { count: 0, sample: vec![], capped: false };
    let start = Instant::now();
    let mut scanned: u64 = 0;
    for src in sources {
        let is_dir = std::fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false);
        if !is_dir {
            continue; // a source file is exempt from patterns
        }
        let mut queue: VecDeque<String> = VecDeque::from([clean_path(src)]);
        while let Some(current) = queue.pop_front() {
            let Ok(rd) = std::fs::read_dir(&current) else { continue };
            for ent in rd.flatten() {
                scanned += 1;
                if scanned & 0xFF == 0
                    && (cancel.load(Ordering::Relaxed) || start.elapsed() > limits.budget || scanned > limits.max_entries)
                {
                    out.capped = true;
                    return out;
                }
                let Some(name) = ent.file_name().to_str().map(str::to_string) else { continue };
                let path = if current == "/" { format!("/{name}") } else { format!("{current}/{name}") };
                if base.judge_node(&path, probe).is_some() {
                    continue; // already left out, with everything below it
                }
                if with.judge_node(&path, probe).is_some() {
                    out.count += 1;
                    if out.sample.len() < MATCH_SAMPLE {
                        out.sample.push(path);
                    }
                    if out.count >= MATCH_CAP {
                        out.capped = true;
                        return out;
                    }
                    continue;
                }
                if ent.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    queue.push_back(path);
                }
            }
        }
    }
    out
}

/// Which scan is current. Starting a scan cancels the previous one; the frontend re-sends
/// whatever is still unresolved, so superseding never loses work.
#[derive(Default)]
pub struct TreeScanState {
    current: Mutex<Option<Arc<AtomicBool>>>,
    next_id: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanEvent {
    scan_id: u64,
    path: String,
    partial: bool,
    complete: bool,
    first_excluded: Option<String>,
    unreadable: bool,
    scanned: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanDoneEvent {
    scan_id: u64,
}

const SCAN_THREADS: usize = 3;

// ── commands ────────────────────────────────────────────────────────────────

/// The tree preview needs Unix path semantics (see plan_rules.rs); Windows keeps the old editor.
#[tauri::command]
pub fn tree_supported() -> bool {
    cfg!(unix)
}

#[tauri::command]
pub async fn tree_kinds(paths: Vec<String>, follow: Option<bool>) -> Result<Vec<&'static str>, String> {
    tauri::async_runtime::spawn_blocking(move || kinds(&paths, follow.unwrap_or(false)))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn tree_list_dir(path: String, limit: Option<usize>) -> Result<TreeListing, String> {
    let limit = limit.unwrap_or(3000).clamp(1, 20_000);
    tauri::async_runtime::spawn_blocking(move || list_dir(&path, limit))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn tree_classify(rules: TreeRulesInput, paths: Vec<String>) -> Result<Classification, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (engine, origins, problems) = real_build(&rules)?;
        let probe = FsProbe::default();
        Ok(Classification { nodes: classify_paths(&engine, &probe, &paths), origins, problems })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Starts scanning `dirs` (each assumed included) on a few worker threads, emitting a
/// `tree:scan` event per folder and `tree:scan-done` at the end. Returns the scan id the
/// events carry; a newer scan supersedes this one.
#[tauri::command]
pub async fn tree_scan_start(
    app: AppHandle,
    state: State<'_, TreeScanState>,
    rules: TreeRulesInput,
    dirs: Vec<String>,
    budget_ms: Option<u64>,
    max_entries: Option<u64>,
) -> Result<u64, String> {
    let (engine, _, _) = real_build(&rules)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut cur = state.current.lock().map_err(|e| e.to_string())?;
        if let Some(prev) = cur.replace(Arc::clone(&cancel)) {
            prev.store(true, Ordering::SeqCst);
        }
    }
    let scan_id = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let limits = ScanLimits {
        budget: Duration::from_millis(budget_ms.unwrap_or(1500).clamp(50, 30_000)),
        max_entries: max_entries.unwrap_or(2_000_000),
    };
    // Fire and forget: results arrive as events.
    #[allow(clippy::let_underscore_future)]
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let probe = FsProbe::default();
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..SCAN_THREADS.min(dirs.len().max(1)) {
                scope.spawn(|| loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(dir) = dirs.get(i) else { break };
                    let o = scan_dir(&engine, &probe, &clean_path(dir), limits, &cancel);
                    if cancel.load(Ordering::Relaxed) && !o.complete {
                        break; // superseded mid-folder: don't report a half-done result
                    }
                    let _ = app.emit(
                        "tree:scan",
                        ScanEvent {
                            scan_id,
                            path: dir.clone(),
                            partial: o.partial,
                            complete: o.complete,
                            first_excluded: o.first_excluded,
                            unreadable: o.unreadable,
                            scanned: o.scanned,
                        },
                    );
                });
            }
        });
        let _ = app.emit("tree:scan-done", ScanDoneEvent { scan_id });
    });
    Ok(scan_id)
}

/// Applies a click (include/exclude) to the plan's sources and patterns and returns the result —
/// or why it can't be done. The edit is verified by the rule engine; see plan_edit.rs.
#[tauri::command]
pub async fn tree_plan_edit(rules: TreeRulesInput, action: EditAction) -> Result<EditOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || {
        apply_edit(
            &rules,
            &action,
            &FsProbe::default(),
            &|p| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()),
            &|name| std::env::var(name).unwrap_or_default(),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "This pattern would leave out N items": a bounded scan comparing the plan with and without it.
#[tauri::command]
pub async fn tree_count_matches(
    rules: TreeRulesInput,
    pattern: String,
    budget_ms: Option<u64>,
) -> Result<MatchPreview, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let pattern = pattern.trim().to_string();
        if pattern.is_empty() || pattern.starts_with('#') {
            return Ok(MatchPreview { count: 0, sample: vec![], capped: false });
        }
        Pattern::parse(&pattern).map_err(|_| format!("`{pattern}` isn't a valid pattern."))?;
        let (base, _, _) = real_build(&rules)?;
        let mut with_rules = rules.clone();
        with_rules.excludes.push(pattern);
        let (with, _, _) = real_build(&with_rules)?;
        let limits = ScanLimits {
            budget: Duration::from_millis(budget_ms.unwrap_or(2000).clamp(50, 30_000)),
            max_entries: 2_000_000,
        };
        Ok(count_matches(&base, &with, &FsProbe::default(), base.sources(), limits, &AtomicBool::new(false)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The files and command line that reproduce this plan's include/exclude behaviour with the
/// plain restic CLI.
#[tauri::command]
pub async fn tree_export(rules: TreeRulesInput) -> Result<ExportOut, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let built = build_rules(
            &rules,
            &|p| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()),
            &|name| std::env::var(name).unwrap_or_default(),
        );
        let mut out = export_rules(&built.rules);
        for p in &built.problems {
            out.notes.push(format!("Left out: {} ({})", p.what, p.error));
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Writes the exported exclude file(s) into `dir` (created if needed), returning the paths.
#[tauri::command]
pub async fn tree_export_write(
    dir: String,
    exclude_file_name: String,
    exclude_file: String,
    iexclude_file_name: Option<String>,
    iexclude_file: Option<String>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
        let mut written = Vec::new();
        let mut put = |name: &str, text: &str| -> Result<(), String> {
            // Only a bare file name: this command must not write anywhere but `dir`.
            if name.is_empty() || name.contains('/') || name.contains('\\') || name == ".." || name == "." {
                return Err(format!("Invalid file name: {name}"));
            }
            let path = dir.join(name);
            std::fs::write(&path, text).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
            written.push(path.to_string_lossy().into_owned());
            Ok(())
        };
        put(&exclude_file_name, &exclude_file)?;
        if let (Some(n), Some(t)) = (iexclude_file_name.as_deref(), iexclude_file.as_deref()) {
            put(n, t)?;
        }
        Ok(written)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn tree_scan_cancel(state: State<'_, TreeScanState>) {
    if let Ok(mut cur) = state.current.lock() {
        if let Some(c) = cur.take() {
            c.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests;
