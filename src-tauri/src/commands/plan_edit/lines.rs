//! Bookkeeping on a plan's exclude-pattern list for tree edits: finding the lines a click wrote
//! (a path's exact rule, its `!` re-include), the one-line `# Tree: ` comments that introduce
//! them, and the carve-out scaffolding (`!A` / `A/*` per level) an undo may take with it.
//!
//! Everything here is text on the list as stored. The rule engine decides what a list *means*;
//! these helpers only decide which lines a click may touch and which lines belong together.

use crate::commands::plan_rules::PathStyle;

/// Starts every comment the tree writes.
pub(crate) const TREE_PREFIX: &str = "# Tree: ";

/// Separates a tree comment's path from its optional note.
const NOTE_SEP: &str = " — ";

/// What the rules a tree comment introduces do to the path it names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Verb {
    Excluded,
    Included,
}

impl Verb {
    fn word(self) -> &'static str {
        match self {
            Verb::Excluded => "excluded",
            Verb::Included => "included",
        }
    }
}

/// The text of `path` as a pattern that matches exactly it: cleaned, then glob-escaped for the
/// platform.
pub(crate) fn exact_rule(style: PathStyle, path: &str) -> String {
    style.escape_glob(&style.clean(path))
}

/// Indices (ascending) of the lines whose trimmed text is `text`.
fn lines_equal_to(lines: &[String], text: &str) -> Vec<usize> {
    lines.iter().enumerate().filter(|(_, l)| l.trim() == text).map(|(i, _)| i).collect()
}

/// The lines that are `path`'s exact rule: not negated, not a comment, not a longer pattern.
pub(crate) fn exact_lines(style: PathStyle, lines: &[String], path: &str) -> Vec<usize> {
    lines_equal_to(lines, &exact_rule(style, path))
}

/// The lines that are `!` plus `path`'s exact rule.
pub(crate) fn reinclude_lines(style: PathStyle, lines: &[String], path: &str) -> Vec<usize> {
    lines_equal_to(lines, &format!("!{}", exact_rule(style, path)))
}

/// A line of text with every control character (a newline above all) replaced by `?`, so it can
/// never end the comment it is written into.
fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

/// The part of a tree comment that identifies its path: `# Tree: excluded /s/a`.
fn comment_head(verb: Verb, path: &str) -> String {
    format!("{TREE_PREFIX}{} {}", verb.word(), one_line(path))
}

/// The comment the tree writes above the rules it adds: `# Tree: excluded <path>` or
/// `# Tree: included <path>`, plus ` — <note>` saying why the rules look as they do. Always one
/// line; a blank note is left out (the stored list is trimmed, which would cut its separator).
pub(crate) fn tree_comment(verb: Verb, path: &str, note: Option<&str>) -> String {
    let mut comment = comment_head(verb, path);
    if let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) {
        comment.push_str(NOTE_SEP);
        comment.push_str(&one_line(note));
    }
    comment
}

/// Is `line` a tree comment for exactly `path` and `verb`? The path must end the line or be
/// followed by the note separator, so the comment for `/s/ab` is not the one for `/s/a`.
pub(crate) fn comment_names(line: &str, verb: Verb, path: &str) -> bool {
    line.strip_prefix(comment_head(verb, path).as_str())
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(NOTE_SEP))
}

/// Removes the lines at `remove` (indices into `lines`; out-of-range ones are ignored), and with
/// each the tree comment for `path` sitting directly above it, if there is one. A comment the
/// user wrote, or one for another path, is never taken.
pub(crate) fn remove_with_comments(lines: &mut Vec<String>, remove: &[usize], path: &str) {
    let mut drop = vec![false; lines.len()];
    for &i in remove.iter().filter(|&&i| i < lines.len()) {
        drop[i] = true;
        let tree_comment_above = i > 0
            && [Verb::Excluded, Verb::Included].iter().any(|&verb| comment_names(&lines[i - 1], verb, path));
        if tree_comment_above {
            drop[i - 1] = true;
        }
    }
    let mut i = 0;
    lines.retain(|_| {
        i += 1;
        !drop[i - 1]
    });
}

/// `A/*`, the rule that excludes what is inside `A` (`A` is a rule's text, e.g. `/s/X`, or a
/// root such as `/` or `C:/`).
fn children_glob(a: &str) -> String {
    format!("{}/*", a.trim_end_matches('/'))
}

/// `A/`, the text every pattern strictly inside `A` starts with.
fn under_prefix(a: &str) -> String {
    format!("{}/", a.trim_end_matches('/'))
}

/// Is `line` a negation of something strictly under `under` (an [`under_prefix`])?
fn negates_under(line: &str, under: &str) -> bool {
    line.trim().strip_prefix('!').is_some_and(|rest| rest.len() > under.len() && rest.starts_with(under))
}

/// Is `lines[idx]` carve-out scaffolding: `!A` directly followed by `A/*`, or a `!A` with a
/// re-include somewhere below it that lives inside `A/`? Such a line holds a carve-out together,
/// so a click on `A` itself must not delete it.
///
/// The comparison is on rule text, which is already in the platform's spelling (`/`-separated,
/// glob-escaped), so `_style` is not consulted; it is kept so every helper takes the platform.
pub(crate) fn is_scaffolding(_style: PathStyle, lines: &[String], idx: usize) -> bool {
    let Some(a) = lines.get(idx).and_then(|l| l.trim().strip_prefix('!')).filter(|a| !a.is_empty()) else {
        return false;
    };
    if lines.get(idx + 1).is_some_and(|next| next.trim() == children_glob(a)) {
        return true;
    }
    let under = under_prefix(a);
    lines[idx + 1..].iter().any(|l| negates_under(l, &under))
}

/// The indices of every adjacent `!A`, `A/*` pair in `lines`.
fn adjacent_pairs(lines: &[String], a: &str) -> Vec<usize> {
    let glob = children_glob(a);
    lines
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0].trim().strip_prefix('!') == Some(a) && w[1].trim() == glob)
        .flat_map(|(i, _)| [i, i + 1])
        .collect()
}

/// The scaffolding left behind once `path`'s own leaf rule is gone from `lines`: walking up from
/// `path`, a level `A` is returned (the indices of its `!A`, `A/*` pair) when no remaining line
/// re-includes something strictly under `A/`. Lines already chosen for removal in this call do
/// not count as remaining, so a chain of single-use levels is returned whole; a level some other
/// carve-out still uses, or one with no such pair, ends the walk. Indices ascend.
pub(crate) fn orphan_scaffolding(style: PathStyle, lines: &[String], path: &str) -> Vec<usize> {
    let mut orphans: Vec<usize> = Vec::new();
    let mut level = style.clean(path);
    loop {
        let parent = style.parent(&level);
        if parent == level {
            break;
        }
        level = parent;
        let a = style.escape_glob(&level);
        let under = under_prefix(&a);
        let still_used = lines.iter().enumerate().any(|(i, l)| !orphans.contains(&i) && negates_under(l, &under));
        let pairs = adjacent_pairs(lines, &a);
        if still_used || pairs.is_empty() {
            break;
        }
        orphans.extend(pairs);
    }
    orphans.sort_unstable();
    orphans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::plan_rules::PathStyle;

    fn v(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn exact_rule_escapes_per_platform() {
        assert_eq!(exact_rule(PathStyle::Unix, "/s/we[ird]/x*y"), "/s/we\\[ird\\]/x\\*y");
        assert_eq!(exact_rule(PathStyle::Windows, "C:/s/we[ird]"), "C:/s/we[[]ird]");
        // The path is cleaned first, in the platform's own way.
        assert_eq!(exact_rule(PathStyle::Unix, "/s//a/"), "/s/a");
        assert_eq!(exact_rule(PathStyle::Windows, "C:\\s\\we[ird]\\"), "C:/s/we[[]ird]");
    }

    #[test]
    fn exact_lines_find_only_plain_exact_lines() {
        let lines = v(&["/s/a", "!/s/a", "# /s/a", "/s/a/*", " /s/a ", "/s/a"]);
        assert_eq!(exact_lines(PathStyle::Unix, &lines, "/s/a"), [0, 4, 5]);
        assert_eq!(reinclude_lines(PathStyle::Unix, &lines, "/s/a"), [1]);
        assert!(exact_lines(PathStyle::Unix, &lines, "/s/b").is_empty());
    }

    #[test]
    fn exact_lines_match_the_escaped_spelling_on_windows() {
        let lines = v(&["C:/s/we[[]ird]", "!C:/s/we[[]ird]", "C:/s/we[ird]"]);
        assert_eq!(exact_lines(PathStyle::Windows, &lines, "C:\\s\\we[ird]"), [0]);
        assert_eq!(reinclude_lines(PathStyle::Windows, &lines, "C:/s/we[ird]"), [1]);
        // On Unix the same path escapes with backslashes, so none of these lines is its rule.
        assert!(exact_lines(PathStyle::Unix, &lines, "C:/s/we[ird]").is_empty());
    }

    #[test]
    fn tree_comments_are_one_line_and_match_only_their_own_path() {
        let c = tree_comment(Verb::Excluded, "/s/a\nb", None);
        assert!(!c.contains('\n'));
        assert!(c.contains("/s/a?b"));
        assert_eq!(tree_comment(Verb::Excluded, "/s/a", None), "# Tree: excluded /s/a");
        assert_eq!(tree_comment(Verb::Included, "/s/a", Some("why")), "# Tree: included /s/a — why");
        assert!(!tree_comment(Verb::Included, "/s/a", Some("x\r\ny")).contains(['\n', '\r']));

        let line = tree_comment(Verb::Included, "/s/a", Some("why"));
        assert!(comment_names(&line, Verb::Included, "/s/a"));
        assert!(!comment_names(&line, Verb::Excluded, "/s/a"));
        assert!(!comment_names(&line, Verb::Included, "/s/ab"));
        assert!(!comment_names("# Tree: nothing", Verb::Included, "/s/a"));
        // A longer path with the same start does not match a shorter one either way round.
        assert!(!comment_names("# Tree: included /s/ab", Verb::Included, "/s/a"));
        assert!(comment_names("# Tree: included /s/a", Verb::Included, "/s/a"));
        // A path with a control character matches the comment written for it.
        assert!(comment_names(&c, Verb::Excluded, "/s/a\nb"));
        // Only the tree's own comments count.
        assert!(!comment_names("# included /s/a", Verb::Included, "/s/a"));
    }

    #[test]
    fn removal_takes_the_adjacent_tree_comment_only() {
        let mut lines = v(&["# my note", "# Tree: excluded /s/a", "/s/a", "*.log"]);
        remove_with_comments(&mut lines, &[2], "/s/a");
        assert_eq!(lines, ["# my note", "*.log"]);

        let mut lines = v(&["# Tree: excluded /s/a", "*.log", "/s/a"]);
        remove_with_comments(&mut lines, &[2], "/s/a");
        assert_eq!(lines, ["# Tree: excluded /s/a", "*.log"]);
    }

    #[test]
    fn removal_leaves_other_paths_comments_and_takes_either_verb() {
        let mut lines = v(&["# Tree: excluded /s/ab", "/s/a", "# Tree: included /s/a — why", "!/s/a", "# Tree: excluded /s/a"]);
        remove_with_comments(&mut lines, &[1, 3], "/s/a");
        assert_eq!(lines, ["# Tree: excluded /s/ab", "# Tree: excluded /s/a"]);

        // Out-of-range indices and an empty list are harmless.
        let mut lines = v(&["a", "b"]);
        remove_with_comments(&mut lines, &[], "/s/a");
        remove_with_comments(&mut lines, &[7], "/s/a");
        assert_eq!(lines, ["a", "b"]);
    }

    #[test]
    fn scaffolding_is_recognised() {
        let lines = v(&["/s/X", "!/s/X", "/s/X/*", "!/s/X/a"]);
        assert!(is_scaffolding(PathStyle::Unix, &lines, 1));
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 3));
        // Only a negation can be scaffolding.
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 0));
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 2));
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 9));

        let lines = v(&["*.tmp", "!/s/t.tmp"]);
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 1));
    }

    #[test]
    fn scaffolding_is_also_a_negation_with_negations_below_it() {
        // `!X` with no `X/*` right after it, but a re-include under it further down.
        let lines = v(&["!/s/X", "*.log", "!/s/X/a"]);
        assert!(is_scaffolding(PathStyle::Unix, &lines, 0));
        // A negation earlier in the list, or only a sibling with the same start, does not count.
        let lines = v(&["!/s/X/a", "!/s/X", "!/s/Xy/a"]);
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 1));
    }

    #[test]
    fn scaffolding_works_at_a_root_and_on_windows() {
        let lines = v(&["!/", "/*", "!/x"]);
        assert!(is_scaffolding(PathStyle::Unix, &lines, 0));
        assert!(!is_scaffolding(PathStyle::Unix, &lines, 2));
        let lines = v(&["!C:/", "C:/*", "!C:/x"]);
        assert!(is_scaffolding(PathStyle::Windows, &lines, 0));
        assert!(!is_scaffolding(PathStyle::Windows, &lines, 2));
        let lines = v(&["!C:/s/we[[]ird]", "C:/s/we[[]ird]/*"]);
        assert!(is_scaffolding(PathStyle::Windows, &lines, 0));
    }

    #[test]
    fn orphan_scaffolding_stops_at_a_level_still_in_use() {
        let lines = v(&["/s/X", "!/s/X", "/s/X/*", "!/s/X/c", "/s/X/c/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/X/c/d"), [1, 2, 3, 4]);

        // A sibling carve-out under `/s/X` keeps that level.
        let lines = v(&["/s/X", "!/s/X", "/s/X/*", "!/s/X/c", "/s/X/c/*", "!/s/X/b"]);
        assert_eq!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/X/c/d"), [3, 4]);
    }

    #[test]
    fn orphan_scaffolding_stops_at_the_first_level_without_a_pair() {
        // No `!/s/X/c` pair: nothing above it is touched, even though `/s/X` has one.
        let lines = v(&["!/s/X", "/s/X/*"]);
        assert!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/X/c/d").is_empty());
        // The pair must be adjacent.
        let lines = v(&["!/s/X", "*.log", "/s/X/*"]);
        assert!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/X/c").is_empty());
        // The leaf itself still being there keeps its parent level in use.
        let lines = v(&["!/s/X", "/s/X/*", "!/s/X/c"]);
        assert!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/X/c").is_empty());
    }

    #[test]
    fn orphan_scaffolding_reaches_a_root_and_works_on_windows() {
        let lines = v(&["!/", "/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Unix, &lines, "/x"), [0, 1]);

        let lines = v(&["C:/s/X", "!C:/s/X", "C:/s/X/*", "!C:/s/X/c", "C:/s/X/c/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Windows, &lines, "C:\\s\\X\\c\\d"), [1, 2, 3, 4]);

        let lines = v(&["!C:/", "C:/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Windows, &lines, "C:/x"), [0, 1]);

        // Escaped names are matched in their escaped spelling.
        let lines = v(&["!C:/s/we[[]ird]", "C:/s/we[[]ird]/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Windows, &lines, "C:/s/we[ird]/f"), [0, 1]);
        let lines = v(&["!/s/we\\[ird\\]", "/s/we\\[ird\\]/*"]);
        assert_eq!(orphan_scaffolding(PathStyle::Unix, &lines, "/s/we[ird]/f"), [0, 1]);
    }
}
