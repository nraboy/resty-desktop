//! Reads `--files-from` list files the way restic does, so the plan editor's tree can show the
//! folders and files they add. Each line of such a file is one more *source* — same as a path
//! given on the command line: exempt from the exclude patterns itself, its contents judged — so
//! the entries simply join the engine's sources (see `PlanRules::list_sources`).
//!
//! What restic does (checked against 0.19.1 with `restic backup --dry-run -vv --files-from`):
//! - a line is trimmed (whitespace, a trailing `\r`, a leading BOM on the file), and blank lines
//!   and lines starting with `#` — even indented ones — are skipped;
//! - what is left goes through Go's `filepath.Glob`; `~` and `$VAR` are **not** expanded, `**` is
//!   just `*` (one path component), and a pattern that matches nothing — including a path that
//!   doesn't exist — is skipped with a warning, not an error;
//! - a relative entry is resolved against restic's working directory, which the editor can't know,
//!   so it is reported and not shown;
//! - every match is a source of its own, a folder is backed up recursively.

use std::collections::HashSet;

use serde::Serialize;

use super::plan_rules::{go_match, PathStyle};
use super::plan_tree::RuleProblem;

/// Entries shown at most: a list of tens of thousands of files would make a tree nobody can read,
/// and every one is a root the engine has to consider.
pub(crate) const MAX_LIST_ENTRIES: usize = 10_000;

/// One source contributed by a list file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListEntry {
    /// The source, in the tree's spelling (`/` separators, `C:/…` on Windows).
    pub path: String,
    /// The list file it came from, as the plan names it.
    pub file: String,
    /// 1-based line in that file.
    pub line: u32,
    /// The line itself — differs from `path` when it was a glob.
    pub pattern: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListFiles {
    pub entries: Vec<ListEntry>,
    /// Lines that match nothing, can't be read or can't be shown (`what` names `file:line`).
    pub problems: Vec<RuleProblem>,
    /// Stopped at `MAX_LIST_ENTRIES`.
    pub truncated: bool,
}

/// The lines of a list file that name something, with their 1-based line numbers.
pub(crate) fn parse_lines(text: &str) -> Vec<(u32, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .enumerate()
        .filter_map(|(i, raw)| {
            let line = raw.trim();
            (!line.is_empty() && !line.starts_with('#')).then(|| (i as u32 + 1, line.to_string()))
        })
        .collect()
}

fn has_meta(path: &str, style: PathStyle) -> bool {
    // On Windows a backslash is a separator, not an escape (and canonical paths have none).
    path.contains(['*', '?', '[']) || (style == PathStyle::Unix && path.contains('\\'))
}

/// Go's `filepath.Split` on a canonical path: the directory with its trailing separator, then the
/// last element.
fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    }
}

/// Go's `cleanGlobPath`: the directory without its trailing separator, a root kept whole.
fn clean_glob_dir(dir: &str) -> &str {
    if dir.is_empty() {
        "."
    } else if super::plan_rules::is_root(dir) {
        dir
    } else {
        &dir[..dir.len() - 1]
    }
}

/// Lists a directory's names, sorted — `None` when it isn't a readable directory (Go's `glob`
/// silently yields nothing in that case).
fn dir_names(dir: &str, style: PathStyle) -> Option<Vec<String>> {
    let native = style.to_native(dir);
    let meta = std::fs::metadata(&native).ok()?; // follows symlinks, like os.Stat
    if !meta.is_dir() {
        return None;
    }
    let mut names: Vec<String> = std::fs::read_dir(&native)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    Some(names)
}

fn glob_in_dir(dir: &str, pattern: &str, style: PathStyle, out: &mut Vec<String>, budget: &mut usize) {
    let Some(names) = dir_names(dir, style) else { return };
    for n in names {
        if *budget == 0 {
            return;
        }
        if go_match(pattern, &n).unwrap_or(false) {
            out.push(style.join(dir, &n));
            *budget -= 1;
        }
    }
}

fn glob_rec(pattern: &str, style: PathStyle, budget: &mut usize) -> Vec<String> {
    if !has_meta(pattern, style) {
        // A plain path: it is a match if it exists (an `lstat`, so a dangling symlink counts).
        return if std::fs::symlink_metadata(style.to_native(pattern)).is_ok() { vec![pattern.to_string()] } else { vec![] };
    }
    let (dir, file) = split(pattern);
    let dir = clean_glob_dir(dir);
    let mut out = Vec::new();
    if !has_meta(dir, style) {
        glob_in_dir(dir, file, style, &mut out, budget);
        return out;
    }
    for d in glob_rec(dir, style, budget) {
        glob_in_dir(&d, file, style, &mut out, budget);
    }
    out
}

/// Go's `filepath.Glob` over a canonical absolute pattern, at most `limit` results. An `Err` is a
/// malformed pattern (restic would refuse to run).
pub(crate) fn glob(pattern: &str, style: PathStyle, limit: usize) -> Result<Vec<String>, String> {
    go_match(pattern, "").map_err(|_| "malformed pattern".to_string())?;
    let mut budget = limit;
    Ok(glob_rec(pattern, style, &mut budget))
}

/// Expands every list file into the sources it adds. `read` reads a list file (injected so tests
/// need no files); globbing and existence checks use the real filesystem.
pub(crate) fn read_list_files(
    files: &[String],
    style: PathStyle,
    read: &dyn Fn(&str) -> std::io::Result<String>,
) -> ListFiles {
    read_with_limit(files, style, read, MAX_LIST_ENTRIES)
}

fn read_with_limit(
    files: &[String],
    style: PathStyle,
    read: &dyn Fn(&str) -> std::io::Result<String>,
    limit: usize,
) -> ListFiles {
    let mut result = ListFiles::default();
    let mut seen: HashSet<String> = HashSet::new();
    'files: for f in files {
        let file = f.trim();
        if file.is_empty() {
            continue;
        }
        let text = match read(file) {
            Ok(t) => t,
            Err(e) => {
                result.problems.push(RuleProblem { what: file.to_string(), error: format!("can't read path-list file: {e}") });
                continue;
            }
        };
        for (n, line) in parse_lines(&text) {
            let at = format!("{file}:{n}");
            let pattern = style.clean(&line);
            if super::plan_rules::root_of(&pattern).is_none() {
                result.problems.push(RuleProblem {
                    what: at,
                    error: format!("`{line}` is a relative path — restic resolves it from the folder it runs in, so it isn't shown here"),
                });
                continue;
            }
            let remaining = limit - result.entries.len();
            let matches = match glob(&pattern, style, remaining + 1) {
                Ok(m) => m,
                Err(e) => {
                    result.problems.push(RuleProblem { what: at, error: format!("`{line}`: {e} — restic won't run with it") });
                    continue;
                }
            };
            if matches.is_empty() {
                result.problems.push(RuleProblem {
                    what: at,
                    error: format!("`{line}` matches nothing — restic skips it with a warning"),
                });
                continue;
            }
            for m in matches {
                let path = style.clean(&m);
                if !seen.insert(path.clone()) {
                    continue;
                }
                if result.entries.len() >= limit {
                    result.truncated = true;
                    result.problems.push(RuleProblem {
                        what: file.to_string(),
                        error: format!("only the first {limit} entries are shown"),
                    });
                    break 'files;
                }
                result.entries.push(ListEntry { path, file: file.to_string(), line: n, pattern: line.clone() });
            }
        }
    }
    result
}

#[tauri::command]
pub async fn tree_list_files(files: Vec<String>) -> Result<ListFiles, String> {
    tauri::async_runtime::spawn_blocking(move || {
        read_list_files(&files, PathStyle::NATIVE, &|p| std::fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()))
    })
    .await
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
