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
//! command line a user would run by hand. The rules a click adds come with a one-line
//! `# Tree: ` comment above them saying what they are for. The only lines an edit deletes are
//! ones that exist for the clicked item alone, so that a second click undoes the first instead of
//! stacking rules: including an item excluded by its own exact line deletes that line; excluding
//! an item a lone `!` re-include brings back deletes that `!` line, and with the last carve-out
//! out of a folder the scaffolding that opened the folder for it. The tree comment directly above
//! a deleted line goes with it. Broader rules, exclude files and the user's own comments are
//! never touched.
//!
//! The patterns that carve one item out of an excluded folder follow the gitignore-style chain
//! restic needs (verified against restic 0.19.1; see docs/backup-plan-tree.md):
//! `!X`, `X/*`, `!X/child`, `X/child/*`, …, `!X/child/…/target`.

use serde::{Deserialize, Serialize};

use self::lines::{
    exact_lines, exact_rule, is_scaffolding, orphan_scaffolding, reinclude_lines, remove_with_comments, tree_comment, Verb,
};
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
    /// `exclude` (with a pattern), `excludeItem` (a click on an included item: exactly it),
    /// `include` (a click on an excluded item) or `includeEverything` (`!path`: the item and
    /// everything inside it that other rules exclude).
    pub kind: String,
    pub path: String,
    /// For `exclude`: the pattern to add (the dialog's choice, possibly edited by hand).
    #[serde(default)]
    pub pattern: Option<String>,
}

/// How many items inside a re-included folder other rules had excluded (they come back with it).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BroughtBack {
    pub count: u64,
    /// The count stopped at the scan's budget: there are at least `count`.
    pub capped: bool,
}

/// The plan's sources and exclude patterns after the edit, and what changed. `added_rules` and
/// `removed_rules` list real patterns only, never the tree comments that go with them.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct EditOutcome {
    pub sources: Vec<String>,
    pub excludes: Vec<String>,
    pub added_rules: Vec<String>,
    pub removed_rules: Vec<String>,
    pub added_sources: Vec<String>,
    pub removed_sources: Vec<String>,
    pub brought_back: Option<BroughtBack>,
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
        "excludeItem" => do_exclude_item(work, &path, probe, read, getenv)?,
        "include" => do_include(work, &path, probe, read, getenv)?,
        "includeEverything" => do_include_everything(work, &path, probe, read, getenv)?,
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
    let is_source = work.sources.iter().any(|s| s == path);
    if !e0.classify(path, probe).included {
        return Err(not_included(&e0, path));
    }

    let mut out = EditOutcome::default();
    let note = format!("with pattern {pattern}");
    // Excluding a source folder by its exact path means "stop backing it up": remove the source
    // (a pattern can't exclude a source itself).
    if is_source && (pattern == path || pattern == work.style.escape_glob(path)) {
        work.sources.retain(|s| s != path);
        out.removed_sources.push(path.to_string());
    } else {
        append_with_comment(&mut work, &mut out, Verb::Excluded, path, Some(&note), vec![pattern.to_string()]);
    }

    let (e1, b1) = build_engine(&work, read, getenv)?;
    let mut after = e1.classify(path, probe);
    if after.included && !out.removed_sources.is_empty() {
        // Still covered by another source (a parent folder): add the pattern as well.
        append_with_comment(&mut work, &mut out, Verb::Excluded, path, Some(&note), vec![pattern.to_string()]);
        let (e2, _) = build_engine(&work, read, getenv)?;
        after = e2.classify(path, probe);
    }
    if after.included {
        return Err(not_excluded(&work, path, pattern, is_source, &b1.origins, &after.reason));
    }
    Ok(finish(work, out))
}

/// Why an item that isn't included can't be excluded.
fn not_included(engine: &Engine, path: &str) -> String {
    let above_source = engine.sources().iter().any(|s| matches!(strip_source(s, path), Some(r) if !r.is_empty()));
    if above_source {
        "This folder isn't backed up itself; exclude the folders inside it instead.".to_string()
    } else {
        "It's already excluded.".to_string()
    }
}

/// Why `pattern` left `path` included. `was_source`: `path` was one of the plan's sources.
fn not_excluded(
    work: &TreeRulesInput,
    path: &str,
    pattern: &str,
    was_source: bool,
    origins: &[RuleOrigin],
    reason: &Option<Reason>,
) -> String {
    if was_source {
        "This folder is a backup source itself, so patterns don't apply to it — remove it from the sources instead.".to_string()
    } else if work.list_sources.iter().any(|s| s == path) {
        "This comes from a path-list file, so patterns can't exclude it — remove or change that line in the file.".to_string()
    } else {
        format!("`{pattern}` doesn't exclude it: {}", explain(origins, reason))
    }
}

/// Is `path` excluded from the plan `work`?
fn is_excluded(work: &TreeRulesInput, path: &str, probe: &dyn Probe, read: ReadFile, getenv: GetEnv) -> Result<bool, String> {
    Ok(!build_engine(work, read, getenv)?.0.classify(path, probe).included)
}

/// Excludes an included item (a click on it), undoing the rules that brought it in where it can:
///
/// 1. Included through a lone `!path` re-include (a tree include, the "everything inside it"
///    option, or a hand-written one): those lines are deleted, newest first, until it is
///    excluded — and if that was the last carve-out out of a folder, the scaffolding that opened
///    the folder for it goes too (see [`remove_orphan_scaffolding`]). A `!path` that is itself
///    scaffolding for a carve-out further down is never deleted (that would strand the carve-out
///    as dead lines). If deleting them all still leaves it included, they weren't what brought
///    it in: they stay, and step 2 applies.
/// 2. Otherwise its exact rule is appended.
///
/// A source is removed from the sources instead, as an exclude by its exact path does.
fn do_exclude_item(
    mut work: TreeRulesInput,
    path: &str,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let style = work.style;
    let rule = exact_rule(style, path);
    if work.sources.iter().any(|s| s == path) {
        return do_exclude(work, path, &rule, probe, read, getenv);
    }
    let (e0, _) = build_engine(&work, read, getenv)?;
    if !e0.classify(path, probe).included {
        return Err(not_included(&e0, path));
    }

    let mut out = EditOutcome::default();
    let before = work.excludes.clone();
    let lone: Vec<usize> = reinclude_lines(style, &before, path)
        .into_iter()
        .filter(|&i| !is_scaffolding(style, &before, i))
        .collect();
    // Newest first: deleting one never moves the lines above it (the tree comment it may take
    // along sits directly above it and is never a `!path` line).
    for &i in lone.iter().rev() {
        out.removed_rules.push(work.excludes[i].trim().to_string());
        remove_with_comments(&mut work.excludes, &[i], path);
        if is_excluded(&work, path, probe, read, getenv)? {
            remove_orphan_scaffolding(&mut work, &mut out, path, probe, read, getenv)?;
            return Ok(finish(work, out));
        }
    }
    work.excludes = before;
    out.removed_rules.clear();

    append_with_comment(&mut work, &mut out, Verb::Excluded, path, None, vec![rule.clone()]);
    let (e1, b1) = build_engine(&work, read, getenv)?;
    let after = e1.classify(path, probe);
    if after.included {
        return Err(not_excluded(&work, path, &rule, false, &b1.origins, &after.reason));
    }
    Ok(finish(work, out))
}

/// Once the last carve-out out of a folder is undone, the scaffolding that opened the folder for
/// it (the `!A`, `A/*` pairs no re-include after them still needs; [`orphan_scaffolding`]) would
/// leave the folder backed up empty (F4). It is deleted, with the tree comment above it, when
/// every folder it opened is excluded again without it, as it was before the carve-out. If one
/// isn't (nothing else excludes it any more), those lines are what keeps its contents out: they
/// stay.
fn remove_orphan_scaffolding(
    work: &mut TreeRulesInput,
    out: &mut EditOutcome,
    path: &str,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<(), String> {
    let style = work.style;
    let orphans = orphan_scaffolding(style, &work.excludes, path);
    if orphans.is_empty() {
        return Ok(());
    }
    let mut without = work.clone();
    remove_with_comments(&mut without.excludes, &orphans, path);
    let (engine, _) = build_engine(&without, read, getenv)?;
    // The folders it opened: the ancestors of `path` whose `!A` is among the orphans.
    let mut level = path.to_string();
    loop {
        let parent = style.parent(&level);
        if parent == level {
            break;
        }
        level = parent;
        let reopen = format!("!{}", exact_rule(style, &level));
        if orphans.iter().any(|&i| work.excludes[i].trim() == reopen) && engine.classify(&level, probe).included {
            return Ok(());
        }
    }
    out.removed_rules.extend(orphans.iter().map(|&i| work.excludes[i].trim().to_string()));
    *work = without;
    Ok(())
}

/// Is `path` included, contents and all? `classify` exempts a source root (and a folder leading to
/// another source) from the patterns, but a pattern matching its own path still hides everything
/// inside it: `judge_node` sees that.
fn fully_included(engine: &Engine, path: &str, probe: &dyn Probe) -> bool {
    engine.classify(path, probe).included && engine.judge_node(path, probe).is_none()
}

/// The engine's word on an include edit before it is returned: `path` must now be included.
fn verify_included(work: &TreeRulesInput, path: &str, probe: &dyn Probe, read: ReadFile, getenv: GetEnv) -> Result<(), String> {
    let (engine, built) = build_engine(work, read, getenv)?;
    let v = engine.classify(path, probe);
    let own = engine.judge_node(path, probe);
    if !v.included || own.is_some() {
        let why = if !v.included { explain(&built.origins, &v.reason) } else { explain(&built.origins, &own) };
        return Err(format!("It can't be included: {why}"));
    }
    Ok(())
}

/// Can the plan's own patterns change this verdict? Only a case-sensitive pattern typed into the
/// plan can be deleted or followed by a re-include that wins; a marker, an exclude-file line
/// (applied after the plan's patterns) or a case-insensitive rule can't be overridden from here.
fn plan_can_override(origins: &[RuleOrigin], reason: &Option<Reason>) -> bool {
    match reason {
        Some(Reason::Pattern { rule_id }) => {
            origins.iter().any(|o| o.id == *rule_id && o.file.is_none() && !o.ignore_case)
        }
        _ => false,
    }
}

/// Is `path` a folder on disk, as restic would see it (a link to a folder is stored as a link,
/// not entered)? Only used to word a comment, so anything unreadable counts as a file.
fn is_folder(style: PathStyle, path: &str) -> bool {
    std::fs::symlink_metadata(style.to_native(path)).is_ok_and(|m| m.is_dir())
}

/// Appends `rules` to the plan's patterns, with the tree comment for `path` directly above them.
fn append_with_comment(work: &mut TreeRulesInput, out: &mut EditOutcome, verb: Verb, path: &str, note: Option<&str>, rules: Vec<String>) {
    work.excludes.push(tree_comment(verb, path, note));
    work.excludes.extend(rules.iter().cloned());
    out.added_rules.extend(rules);
}

/// Includes an excluded item. Excluded by its own exact line(s): those are deleted, newest first,
/// until it is included. Still excluded (a broader rule also matches it): `!path` re-includes just
/// this item, and the deleted lines stay deleted (they did nothing). Inside an excluded folder: a
/// carve-out from the highest still-excluded node down — never the whole chain from the folder
/// again, whose `X/*` would re-exclude an earlier carve-out from it. Outside every source: it
/// becomes a source.
fn do_include(
    mut work: TreeRulesInput,
    path: &str,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let style = work.style;
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

    // Something still rejects it (or its contents). Only the plan's own patterns can be changed
    // from here: a marker, an exclude-file line or a case-insensitive rule is refused with why.
    let reason = v1.reason.clone().or(own);
    if !plan_can_override(&b1.origins, &reason) {
        return Err(format!("It can't be included: {}", explain(&b1.origins, &reason)));
    }
    match v1.at.as_deref().filter(|x| *x != path) {
        // Inside the excluded folder `x`: carve it out, leaving the rest of `x` excluded. Its own
        // exact lines stay (the chain's `!path` comes after them).
        Some(x) => {
            let note = format!("out of the excluded {x}: reopen it, exclude what is inside, bring this back");
            append_with_comment(&mut work, &mut out, Verb::Included, path, Some(&note), carve_out_chain(style, x, path));
        }
        // Excluded at itself.
        None => {
            let mut included = false;
            while !included {
                let Some(&i) = exact_lines(style, &work.excludes, path).last() else { break };
                out.removed_rules.push(work.excludes[i].trim().to_string());
                remove_with_comments(&mut work.excludes, &[i], path);
                included = fully_included(&build_engine(&work, read, getenv)?.0, path, probe);
            }
            if !included {
                let note = if is_folder(style, path) {
                    "a broader rule excludes it; this re-includes only this item and everything inside it"
                } else {
                    "a broader rule excludes it; this re-includes only this item"
                };
                let rule = format!("!{}", exact_rule(style, path));
                append_with_comment(&mut work, &mut out, Verb::Included, path, Some(note), vec![rule]);
            }
        }
    }
    verify_included(&work, path, probe, read, getenv)?;
    Ok(finish(work, out))
}

/// `!path`: the item and everything inside it that other rules exclude (restic lets a `!` win over
/// every earlier rule about anything below it). Offered on a partly included folder too, so
/// "already included" is no refusal here; its exact lines stay (the `!` comes after them).
fn do_include_everything(
    mut work: TreeRulesInput,
    path: &str,
    probe: &dyn Probe,
    read: ReadFile,
    getenv: GetEnv,
) -> Result<EditOutcome, String> {
    let style = work.style;
    let (e0, _) = build_engine(&work, read, getenv)?;
    if matches!(e0.classify(path, probe).reason, Some(Reason::OutsideSources)) {
        return Err("It isn't inside a backup source.".to_string());
    }
    let mut out = EditOutcome::default();
    let rule = format!("!{}", exact_rule(style, path));
    append_with_comment(
        &mut work,
        &mut out,
        Verb::Included,
        path,
        Some("and everything inside it that other rules exclude"),
        vec![rule],
    );
    verify_included(&work, path, probe, read, getenv)?;
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

// Rule-list bookkeeping for tree edits: exact lines, `!` re-includes, tree comments, scaffolding.
mod lines;

#[cfg(test)]
mod tests;
