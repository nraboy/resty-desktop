//! Turns a click in the plan tree ("include this", "exclude that") into edits to the plan's
//! sources and exclude patterns, and verifies every edit with the rule engine before it is
//! returned: an edit is only offered if, once applied, the clicked item really is included or
//! excluded as asked. That check is what stops the tree from writing rules that do nothing —
//! for instance a re-include that restic would never see because an ancestor is excluded, or
//! one that an exclude file (applied after the plan's own patterns) would re-exclude.
//!
//! Everything generated is a plain restic pattern (ordered `--exclude` lines, `!` re-includes,
//! source paths), appended after the existing ones so it takes precedence. That is what keeps
//! the GUI and the restic CLI in step: [`export_rules`] renders the same rules as the files and
//! command line a user would run by hand.
//!
//! The patterns that carve one item out of an excluded folder follow the gitignore-style chain
//! restic needs (verified against restic 0.19.1; see docs/backup-plan-tree.md):
//! `!X`, `X/*`, `!X/child`, `X/child/*`, …, `!X/child/…/target`.

use serde::{Deserialize, Serialize};

use super::plan_rules::{strip_source, Engine, PathStyle, Pattern, PlanRules, Probe, Reason};
use super::plan_tree::{build_rules, BuiltRules, RuleOrigin, TreeRulesInput};

pub(crate) type ReadFile<'a> = &'a dyn Fn(&str) -> std::io::Result<String>;
pub(crate) type GetEnv<'a> = &'a dyn Fn(&str) -> String;

/// Escapes the characters Go's `filepath.Match` treats specially so a literal path can be used
/// as a pattern, Unix-style (`\ [ ] * ?` get a backslash). `!` is not an issue: patterns
/// generated from a path start with `/` or a drive letter. See [`PathStyle::escape_glob`] for the
/// Windows form.
#[cfg(test)]
pub fn escape_glob(s: &str) -> String {
    PathStyle::Unix.escape_glob(s)
}

/// What the user asked for in the tree.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditAction {
    /// `exclude` or `include`.
    pub kind: String,
    pub path: String,
    /// For `exclude`: the pattern to add (the dialog's choice, possibly edited by hand).
    #[serde(default)]
    pub pattern: Option<String>,
    /// For `include` below an excluded folder: `item` (just this, the default) or `folder`
    /// (the whole excluded folder comes back).
    #[serde(default)]
    pub scope: Option<String>,
}

/// The plan's sources and exclude patterns after the edit, and what changed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    pub sources: Vec<String>,
    pub excludes: Vec<String>,
    pub added_rules: Vec<String>,
    pub added_sources: Vec<String>,
    pub removed_sources: Vec<String>,
}

fn build_engine(
    input: &TreeRulesInput,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<(Engine, BuiltRules), String> {
    let built = build_rules(input, read, getenv);
    let engine = Engine::new(&built.rules)?;
    Ok((engine, built))
}

/// A sentence saying why something is excluded, for the cases an edit can't fix.
fn explain(origins: &[RuleOrigin], reason: &Option<Reason>) -> String {
    match reason {
        Some(Reason::Pattern { rule_id }) => match origins.iter().find(|o| o.id == *rule_id) {
            Some(o) if o.file.is_some() => format!(
                "it is excluded by `{}` (line {} of {}). Patterns in exclude files are applied after the plan's own patterns, so they can't be overridden from here — edit that file.",
                o.pattern,
                o.line.unwrap_or(0),
                o.file.as_deref().unwrap_or("")
            ),
            Some(o) if o.ignore_case => format!(
                "it is excluded by the case-insensitive pattern `{}`, which a re-include pattern can't cancel.",
                o.pattern
            ),
            Some(o) => format!("it is excluded by `{}`.", o.pattern),
            None => "it is excluded by a pattern.".to_string(),
        },
        Some(Reason::Marker { spec }) => format!(
            "a `{}` file in its folder excludes everything beside it — remove that file or the marker setting.",
            spec.split(':').next().unwrap_or(spec)
        ),
        _ => "it isn't inside a backup source.".to_string(),
    }
}

/// The re-include chain that carves `target` out of the excluded folder `x` while leaving the
/// rest of `x` excluded: `!x`, then for each step down `cur/*` (exclude what's inside) and
/// `!cur/next` (bring the next step back), ending in `!target`.
pub(crate) fn carve_out_chain(style: PathStyle, x: &str, target: &str) -> Vec<String> {
    let rel = strip_source(target, x).unwrap_or("");
    let mut rules = vec![format!("!{}", style.escape_glob(x))];
    let mut cur = x.to_string();
    for comp in rel.split('/').filter(|c| !c.is_empty()) {
        rules.push(format!("{}/*", style.escape_glob(&cur).trim_end_matches('/')));
        cur = style.join(&cur, comp);
        rules.push(format!("!{}", style.escape_glob(&cur)));
    }
    rules
}

/// Applies `action` to `input`, returning the new sources and patterns — or an explanation of
/// why the click can't be honoured. Nothing is returned that the engine doesn't confirm.
pub(crate) fn apply_edit(
    input: &TreeRulesInput,
    action: &EditAction,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let style = input.style;
    let path = style.clean(action.path.trim());
    let mut work = input.clone();
    let mut sources: Vec<String> = Vec::new();
    for s in &input.sources {
        let c = style.clean(s.trim());
        if !s.trim().is_empty() && !sources.contains(&c) {
            sources.push(c);
        }
    }
    work.sources = sources;
    // Sources from path-list files are the list file's, not the plan's: they take part in every
    // check (they are sources to the engine) but are never added, removed or rewritten here.
    work.list_sources = input.list_sources.iter().map(|s| style.clean(s.trim())).collect();
    let outcome = match action.kind.as_str() {
        "exclude" => do_exclude(work, &path, action.pattern.as_deref().unwrap_or(""), probe, read, getenv)?,
        "include" => do_include(work, &path, action.scope.as_deref() == Some("folder"), probe, read, getenv)?,
        other => return Err(format!("Unknown edit: {other}")),
    };
    Ok(outcome)
}

/// The outcome in the spelling the plan stores: sources in the OS's own form (`C:\Users\me` on
/// Windows) while patterns stay in the `/` form restic accepts everywhere.
fn finish(work: TreeRulesInput, mut out: EditOutcome) -> EditOutcome {
    let style = work.style;
    out.sources = work.sources.iter().map(|s| style.to_native(s)).collect();
    out.added_sources = out.added_sources.iter().map(|s| style.to_native(s)).collect();
    out.removed_sources = out.removed_sources.iter().map(|s| style.to_native(s)).collect();
    out.excludes = work.excludes;
    out
}

fn do_exclude(
    mut work: TreeRulesInput,
    path: &str,
    pattern: &str,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("Enter a pattern.".to_string());
    }
    if pattern.starts_with('!') {
        return Err("A pattern starting with ! re-includes something; use Include for that.".to_string());
    }
    if pattern.starts_with('#') {
        return Err("A pattern can't start with # (it would be read as a comment).".to_string());
    }
    Pattern::parse_styled(pattern, work.style).map_err(|_| format!("`{pattern}` isn't a valid pattern."))?;

    let (e0, _) = build_engine(&work, read, getenv)?;
    let before = e0.classify(path, probe);
    let is_source = work.sources.iter().any(|s| s == path);
    if !before.included {
        let above_source = e0.sources().iter().any(|s| matches!(strip_source(s, path), Some(r) if !r.is_empty()));
        return Err(if above_source {
            "This folder isn't backed up itself; exclude the folders inside it instead.".to_string()
        } else {
            "It's already excluded.".to_string()
        });
    }

    let mut out = EditOutcome::default();
    // Excluding a source folder by its exact path means "stop backing it up": remove the source
    // (a pattern can't exclude a source itself).
    if is_source && (pattern == path || pattern == work.style.escape_glob(path)) {
        work.sources.retain(|s| s != path);
        out.removed_sources.push(path.to_string());
    } else {
        work.excludes.push(pattern.to_string());
        out.added_rules.push(pattern.to_string());
    }

    let (e1, b1) = build_engine(&work, read, getenv)?;
    let mut after = e1.classify(path, probe);
    if after.included && !out.removed_sources.is_empty() {
        // Still covered by another source (a parent folder): add the pattern as well.
        work.excludes.push(pattern.to_string());
        out.added_rules.push(pattern.to_string());
        let (e2, _) = build_engine(&work, read, getenv)?;
        after = e2.classify(path, probe);
    }
    if after.included {
        return Err(if is_source {
            "This folder is a backup source itself, so patterns don't apply to it — remove it from the sources instead.".to_string()
        } else if work.list_sources.iter().any(|s| s == path) {
            "This comes from a path-list file, so patterns can't exclude it — remove or change that line in the file.".to_string()
        } else {
            format!("`{pattern}` doesn't exclude it: {}", explain(&b1.origins, &after.reason))
        });
    }
    Ok(finish(work, out))
}

fn do_include(
    mut work: TreeRulesInput,
    path: &str,
    whole_folder: bool,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let (e0, _) = build_engine(&work, read, getenv)?;
    let v0 = e0.classify(path, probe);
    // A source root is exempt from the patterns itself, but they can still hide everything
    // inside it (a name pattern matching the root's own path): `judge_node` sees that.
    if v0.included && e0.judge_node(path, probe).is_none() {
        return Err("It's already included.".to_string());
    }

    let mut out = EditOutcome::default();
    if matches!(v0.reason, Some(Reason::OutsideSources)) {
        // Not under any source: make it one, absorbing sources nested inside it.
        let nested: Vec<String> = work
            .sources
            .iter()
            .filter(|s| matches!(strip_source(s, path), Some(r) if !r.is_empty()))
            .cloned()
            .collect();
        work.sources.retain(|s| !nested.contains(s));
        out.removed_sources = nested;
        work.sources.push(path.to_string());
        out.added_sources.push(path.to_string());
    }

    let (e1, b1) = build_engine(&work, read, getenv)?;
    let v1 = e1.classify(path, probe);
    let own = e1.judge_node(path, probe);
    if v1.included && own.is_none() {
        return Ok(finish(work, out));
    }

    // Something still rejects it (or its contents): re-include with patterns. Markers can't be
    // overridden by patterns at all.
    let reason = v1.reason.clone().or(own.clone());
    if let Some(Reason::Marker { .. }) = reason {
        return Err(format!("It can't be included: {}", explain(&b1.origins, &reason)));
    }
    let x = v1.at.clone().unwrap_or_else(|| path.to_string());
    let rules = if whole_folder || x == path {
        vec![format!("!{}", work.style.escape_glob(&x))]
    } else {
        carve_out_chain(work.style, &x, path)
    };
    for r in &rules {
        work.excludes.push(r.clone());
        out.added_rules.push(r.clone());
    }

    let (e2, b2) = build_engine(&work, read, getenv)?;
    let v2 = e2.classify(path, probe);
    let own2 = e2.judge_node(path, probe);
    if !v2.included || own2.is_some() {
        let why = if !v2.included { explain(&b2.origins, &v2.reason) } else { explain(&b2.origins, &own2) };
        return Err(format!("It can't be included: {why}"));
    }
    Ok(finish(work, out))
}

// ── export ──────────────────────────────────────────────────────────────────

/// The files and command line that reproduce a plan's include/exclude behaviour with the plain
/// restic CLI.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportOut {
    pub exclude_file_name: String,
    pub exclude_file: String,
    pub iexclude_file_name: Option<String>,
    pub iexclude_file: Option<String>,
    pub command: String,
    pub notes: Vec<String>,
}

pub const EXCLUDE_FILE_NAME: &str = "restic-excludes.txt";
pub const IEXCLUDE_FILE_NAME: &str = "restic-iexcludes.txt";

/// A pattern as one line of an exclude file: restic expands `$VAR` on every line of such a
/// file, so a literal `$` is written `$$`.
fn file_line(pattern: &str) -> String {
    pattern.replace('$', "$$")
}

/// Quotes one argument so the shell the command is pasted into reads it back as exactly `s`, or
/// `None` when no quoting can promise that. The shell is POSIX `sh` on Unix and PowerShell on
/// Windows; in both, single quotes are fully literal (no `$`, backtick or `%` expansion), so
/// everything but a conservative set of plain characters goes inside them, with `'` doubled
/// (`''`; see `POWERSHELL_QUOTES`) in PowerShell and written `'\''` in `sh`. Plain characters exclude the ones PowerShell
/// treats specially even unquoted (`,` builds an array, a leading `@` splats, `%` and `$` expand).
///
/// PowerShell reads the typographic single quotes (U+2018 ‘, U+2019 ’, U+201A ‚, U+201B ‛) as
/// single-quote characters too — a folder called `it’s` would end the literal early — so inside
/// its single quotes every one of them is doubled, like the ASCII one.
const POWERSHELL_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// A control character (newline, escape…, and the Unicode line/paragraph separators) is refused
/// everywhere — pasted into a terminal it can do more than name a file — and so is `"` on Windows,
/// which Windows PowerShell mangles when passing an argument to a native program. The caller then
/// omits the command and says why. A Windows path can't contain `"`; only a hand-typed marker name
/// could.
fn shell_quote(style: PathStyle, s: &str) -> Option<String> {
    let unsafe_char = |c: char| {
        c.is_control() || c == '\u{2028}' || c == '\u{2029}' || (style == PathStyle::Windows && c == '"')
    };
    if s.chars().any(unsafe_char) {
        return None;
    }
    let plain = |c: char| match style {
        // A backslash is the shell's escape on Unix, an ordinary path character on Windows.
        PathStyle::Unix => c.is_ascii_alphanumeric() || "/._-:=+@%,".contains(c),
        PathStyle::Windows => c.is_ascii_alphanumeric() || "/\\._-:+".contains(c),
    };
    if !s.is_empty() && s.chars().all(plain) {
        return Some(s.to_string());
    }
    Some(match style {
        PathStyle::Unix => format!("'{}'", s.replace('\'', "'\\''")),
        PathStyle::Windows => {
            let mut out = String::from("'");
            for c in s.chars() {
                if POWERSHELL_QUOTES.contains(&c) {
                    out.push(c);
                }
                out.push(c);
            }
            out.push('\'');
            out
        }
    })
}

/// Renders a plan's rules as restic CLI inputs: one ordered `--exclude-file` (flag patterns and
/// every exclude file's lines, in the order restic evaluates them), a separate
/// `--iexclude-file` for case-insensitive rules, the marker flags, and the sources — so
/// `restic backup` with them includes exactly what the GUI does. The repository comes from the
/// usual environment (`RESTIC_REPOSITORY`/`RESTIC_PASSWORD`); no secret is written out.
pub(crate) fn export_rules(rules: &PlanRules) -> ExportOut {
    let mut cs = String::from(
        "# Exclude patterns for `restic backup --exclude-file`, generated by Resty Desktop.\n\
         # One pattern per line, evaluated top to bottom; a leading ! re-includes. Order matters.\n",
    );
    let mut ci = String::from(
        "# Case-insensitive exclude patterns for `restic backup --iexclude-file`, generated by Resty Desktop.\n",
    );
    let (mut n_cs, mut n_ci) = (0, 0);
    for r in &rules.rules {
        if r.ignore_case {
            ci.push_str(&file_line(&r.pattern));
            ci.push('\n');
            n_ci += 1;
        } else {
            cs.push_str(&file_line(&r.pattern));
            cs.push('\n');
            n_cs += 1;
        }
    }

    let mut cmd = vec!["restic".to_string(), "backup".to_string()];
    if n_cs > 0 {
        cmd.push("--exclude-file".to_string());
        cmd.push(EXCLUDE_FILE_NAME.to_string());
    }
    if n_ci > 0 {
        cmd.push("--iexclude-file".to_string());
        cmd.push(IEXCLUDE_FILE_NAME.to_string());
    }
    // An argument that can't be quoted safely (see `shell_quote`) leaves the command out rather
    // than printing one that does something other than what the plan says.
    let mut unquotable: Vec<String> = Vec::new();
    let mut quote = |arg: &str, cmd: &mut Vec<String>| match shell_quote(rules.style, arg) {
        Some(q) => cmd.push(q),
        None => unquotable.push(arg.escape_debug().to_string()),
    };
    for m in &rules.markers {
        cmd.push("--exclude-if-present".to_string());
        quote(m, &mut cmd);
    }
    if rules.exclude_caches {
        cmd.push("--exclude-caches".to_string());
    }
    for f in &rules.files_from {
        cmd.push("--files-from".to_string());
        quote(f, &mut cmd);
    }
    for s in &rules.sources {
        quote(&rules.style.to_native(&rules.style.clean(s)), &mut cmd);
    }

    let mut notes = vec![
        "The repository and password come from RESTIC_REPOSITORY and RESTIC_PASSWORD (or add -r); nothing secret is written to the files.".to_string(),
        "Other plan settings (tags, bandwidth limits, compression) are not part of what gets included and aren't in this command.".to_string(),
    ];
    if n_ci > 0 {
        notes.push("Case-insensitive patterns are in their own file because restic keeps them in a separate list.".to_string());
    }
    if !rules.files_from.is_empty() {
        notes.push("Path-list files are passed as --files-from; restic reads them when it runs, so use absolute paths inside them.".to_string());
    }
    if rules.style == PathStyle::Windows {
        notes.push("The command is written for PowerShell. In cmd.exe, replace the single quotes with double quotes.".to_string());
    }
    let command = if unquotable.is_empty() {
        cmd.join(" ")
    } else {
        notes.push(format!(
            "No command is shown: {} can't be written safely for a shell (it contains a control character or a double quote). Use the exclude file and pass the sources to restic yourself.",
            unquotable.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ")
        ));
        String::new()
    };
    ExportOut {
        exclude_file_name: EXCLUDE_FILE_NAME.to_string(),
        exclude_file: cs,
        iexclude_file_name: (n_ci > 0).then(|| IEXCLUDE_FILE_NAME.to_string()),
        iexclude_file: (n_ci > 0).then_some(ci),
        command,
        notes,
    }
}

#[cfg(test)]
mod tests;
