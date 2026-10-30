use std::collections::{HashMap, HashSet};

use super::lines::{comment_names, Verb};
use super::scan::{count_brought_back, find_collateral, COLLATERAL_LIMITS};
use super::*;
use crate::commands::plan_rules::{FsProbe, Marker, PathStyle};

#[derive(Default)]
struct MockProbe {
    markers: HashSet<(String, String)>,
}

impl Probe for MockProbe {
    fn has_marker(&self, dir: &str, marker: &Marker) -> bool {
        self.markers.contains(&(dir.to_string(), marker.name.clone()))
    }
}

fn no_read(_: &str) -> std::io::Result<String> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
}

fn no_env(_: &str) -> String {
    String::new()
}

fn input(sources: &[&str], excludes: &[&str]) -> TreeRulesInput {
    TreeRulesInput {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        excludes: excludes.iter().map(|s| s.to_string()).collect(),
        style: PathStyle::Unix,
        ..Default::default()
    }
}

fn exclude(path: &str, pattern: &str) -> EditAction {
    EditAction { kind: "exclude".into(), path: path.into(), pattern: Some(pattern.into()) }
}

fn include(path: &str) -> EditAction {
    EditAction { kind: "include".into(), path: path.into(), pattern: None }
}

fn include_everything(path: &str) -> EditAction {
    EditAction { kind: "includeEverything".into(), path: path.into(), pattern: None }
}

fn exclude_item(path: &str) -> EditAction {
    EditAction { kind: "excludeItem".into(), path: path.into(), pattern: None }
}

/// The real patterns of a rule list, comment lines dropped.
fn patterns(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).filter(|l| !l.trim_start().starts_with('#')).collect()
}

fn edit(i: &TreeRulesInput, a: &EditAction) -> Result<EditOutcome, String> {
    apply_edit(i, a, &MockProbe::default(), &no_read, &no_env)
}

/// What every tree edit promises about its rule list: the added and removed rules are real
/// patterns, never comments; rules it adds go at the end with the tree comment naming `p`
/// directly above the first of them (`included` when they start with a `!`, `excluded`
/// otherwise); an edit that only deletes lines leaves no tree comment for `p` behind.
fn assert_tree_comment(o: &EditOutcome, p: &str) {
    assert!(o.added_rules.iter().chain(&o.removed_rules).all(|r| !r.trim_start().starts_with('#')), "{o:?}");
    let names_p = |l: &String| comment_names(l, Verb::Included, p) || comment_names(l, Verb::Excluded, p);
    if o.added_rules.is_empty() {
        assert!(!o.excludes.iter().any(names_p), "a tree comment for {p} is left: {:?}", o.excludes);
        return;
    }
    let start = o.excludes.len() - o.added_rules.len();
    assert_eq!(o.excludes[start..], o.added_rules[..], "the added rules end the list");
    let verb = if o.added_rules[0].starts_with('!') { Verb::Included } else { Verb::Excluded };
    assert!(start > 0 && comment_names(&o.excludes[start - 1], verb, p), "no {verb:?} tree comment for {p} above {:?}", o.excludes);
}

/// One include edit on a plan with source `/s` and `before` as its patterns: the patterns after
/// it (comments aside) are `after`, the clicked path is included, and the comments are in place.
fn include_row(before: &[&str], action: EditAction, after: &[&str]) -> (TreeRulesInput, EditOutcome) {
    let i = input(&["/s"], before);
    let o = edit(&i, &action).unwrap_or_else(|e| panic!("{} {} on {before:?}: {e}", action.kind, action.path));
    assert_eq!(patterns(&o.excludes), after, "{} {} on {before:?}", action.kind, action.path);
    assert!(included_after(&i, &o, &action.path), "{} {} on {before:?}: {:?}", action.kind, action.path, o.excludes);
    assert_tree_comment(&o, &action.path);
    (i, o)
}

/// The plan `i` once `o` is applied to it: the next click starts from this.
fn applied(i: &TreeRulesInput, o: &EditOutcome) -> TreeRulesInput {
    TreeRulesInput { sources: o.sources.clone(), excludes: o.excludes.clone(), ..i.clone() }
}

/// Is `path` included in the plan `i`?
fn included(i: &TreeRulesInput, path: &str) -> bool {
    let (e, _) = build_engine(i, &no_read, &no_env).unwrap();
    e.classify(path, &MockProbe::default()).included
}

/// Is `path` included after applying `outcome` to `i`?
fn included_after(i: &TreeRulesInput, o: &EditOutcome, path: &str) -> bool {
    included(&applied(i, o), path)
}

/// `i` after each of `actions` in turn, every one of which must succeed.
fn edits(i: &TreeRulesInput, actions: &[EditAction]) -> TreeRulesInput {
    actions.iter().fold(i.clone(), |cur, a| {
        let o = edit(&cur, a).unwrap_or_else(|e| panic!("{} {} on {:?}: {e}", a.kind, a.path, cur.excludes));
        applied(&cur, &o)
    })
}

// ── escaping and the chain ──────────────────────────────────────────────────

#[test]
fn escape_glob_escapes_exactly_the_glob_metacharacters() {
    assert_eq!(escape_glob("/a/b c/d.txt"), "/a/b c/d.txt");
    assert_eq!(escape_glob("/a/we[ird]/x*y?"), "/a/we\\[ird\\]/x\\*y\\?");
    assert_eq!(escape_glob("back\\slash"), "back\\\\slash");
}

#[test]
fn the_carve_out_chain_is_the_gitignore_style_sequence() {
    assert_eq!(carve_out_chain(PathStyle::Unix, "/s/a", "/s/a"), ["!/s/a"]);
    assert_eq!(
        carve_out_chain(PathStyle::Unix, "/s/a", "/s/a/b/c"),
        ["!/s/a", "/s/a/*", "!/s/a/b", "/s/a/b/*", "!/s/a/b/c"]
    );
    assert_eq!(carve_out_chain(PathStyle::Unix, "/", "/x"), ["!/", "/*", "!/x"]);
}

// ── exclude ─────────────────────────────────────────────────────────────────

#[test]
fn excluding_a_folder_by_its_exact_path_adds_one_pattern() {
    let i = input(&["/s"], &[]);
    let o = edit(&i, &exclude("/s/a", "/s/a")).unwrap();
    assert_eq!(o.added_rules, ["/s/a"]);
    assert_eq!(o.excludes, ["# Tree: excluded /s/a — with pattern /s/a", "/s/a"]);
    assert_tree_comment(&o, "/s/a");
    assert!(!included_after(&i, &o, "/s/a/x"));
    assert!(included_after(&i, &o, "/s/b"));
}

#[test]
fn excluding_by_name_hits_every_folder_of_that_name() {
    let i = input(&["/s"], &[]);
    let o = edit(&i, &exclude("/s/a/node_modules", "node_modules")).unwrap();
    // The comment names the clicked item and the pattern chosen for it.
    assert_eq!(o.excludes, ["# Tree: excluded /s/a/node_modules — with pattern node_modules", "node_modules"]);
    assert_tree_comment(&o, "/s/a/node_modules");
    assert!(!included_after(&i, &o, "/s/a/node_modules"));
    assert!(!included_after(&i, &o, "/s/c/deep/node_modules/y.js"));
    assert!(included_after(&i, &o, "/s/c/deep/code.py"));
}

#[test]
fn the_new_rule_goes_last_so_it_beats_earlier_reincludes() {
    let i = input(&["/s"], &["/s/*", "!/s/a"]);
    let o = edit(&i, &exclude("/s/a/keep.txt", "/s/a/keep.txt")).unwrap();
    assert_eq!(o.excludes, ["/s/*", "!/s/a", "# Tree: excluded /s/a/keep.txt — with pattern /s/a/keep.txt", "/s/a/keep.txt"]);
    assert!(!included_after(&i, &o, "/s/a/keep.txt"));
    assert!(included_after(&i, &o, "/s/a"));
}

#[test]
fn excluding_a_source_by_its_exact_path_removes_the_source() {
    let i = input(&["/s", "/t"], &[]);
    let o = edit(&i, &exclude("/t", "/t")).unwrap();
    assert_eq!((o.sources.as_slice(), o.removed_sources.as_slice()), (&["/s".to_string()][..], &["/t".to_string()][..]));
    assert!(o.added_rules.is_empty());
    assert!(!included_after(&i, &o, "/t"));
}

#[test]
fn a_source_also_covered_by_a_parent_source_gets_the_pattern_too() {
    let i = input(&["/s", "/s/a"], &[]);
    let o = edit(&i, &exclude("/s/a", "/s/a")).unwrap();
    assert_eq!(o.removed_sources, ["/s/a"]);
    assert_eq!(o.added_rules, ["/s/a"]);
    assert_tree_comment(&o, "/s/a");
    assert!(!included_after(&i, &o, "/s/a/x"));
}

#[test]
fn a_name_pattern_cannot_exclude_a_source_itself() {
    let i = input(&["/s/node_modules"], &[]);
    let err = edit(&i, &exclude("/s/node_modules", "node_modules")).unwrap_err();
    assert!(err.contains("backup source itself"), "{err}");
}

#[test]
fn bad_exclude_requests_are_refused_with_a_reason() {
    let i = input(&["/s"], &["node_modules"]);
    assert!(edit(&i, &exclude("/s/a", "")).unwrap_err().contains("Enter a pattern"));
    assert!(edit(&i, &exclude("/s/a", "!/s/a")).unwrap_err().contains("Include"));
    assert!(edit(&i, &exclude("/s/a", "# note")).unwrap_err().contains("comment"));
    assert!(edit(&i, &exclude("/s/a", "[oops")).unwrap_err().contains("valid"));
    assert!(edit(&i, &exclude("/s/a/node_modules", "/s/a/node_modules")).unwrap_err().contains("already excluded"));
    assert!(edit(&i, &exclude("/", "/")).unwrap_err().contains("isn't backed up itself"));
    // A pattern that doesn't actually match the clicked item is refused, not silently added.
    assert!(edit(&i, &exclude("/s/a", "/s/b")).unwrap_err().contains("doesn't exclude it"));
}

#[test]
fn paths_with_glob_characters_round_trip_through_an_escaped_pattern() {
    let i = input(&["/s"], &[]);
    let path = "/s/we[ird]*dir";
    let o = edit(&i, &exclude(path, &escape_glob(path))).unwrap();
    assert!(!included_after(&i, &o, path));
    assert!(!included_after(&i, &o, &format!("{path}/inner")));
    // The look-alikes the unescaped glob would have matched stay included.
    assert!(included_after(&i, &o, "/s/weirdXdir"));
    assert!(included_after(&i, &o, "/s/we"));
}

// ── include ─────────────────────────────────────────────────────────────────

#[test]
fn including_something_outside_every_source_adds_a_source_and_absorbs_nested_ones() {
    let i = input(&["/home/u/code/app", "/home/u/code/lib"], &[]);
    let o = edit(&i, &include("/home/u/code")).unwrap();
    assert_eq!(o.added_sources, ["/home/u/code"]);
    assert_eq!(o.removed_sources, ["/home/u/code/app", "/home/u/code/lib"]);
    assert_eq!(o.sources, ["/home/u/code"]);
    assert!(included_after(&i, &o, "/home/u/code/app/x"));
    // Adding a source adds no rules, so no comment either.
    assert!(o.excludes.is_empty() && o.added_rules.is_empty() && o.removed_rules.is_empty());

    let o = edit(&input(&["/s"], &[]), &include("/elsewhere/notes.txt")).unwrap();
    assert_eq!(o.sources, ["/s", "/elsewhere/notes.txt"]);
}

#[test]
fn including_an_item_excluded_by_its_own_exact_line_deletes_that_line() {
    let (_, o) = include_row(&["/s/a/c-1.png"], include("/s/a/c-1.png"), &[]);
    assert_eq!(o.removed_rules, ["/s/a/c-1.png"]);
    assert!(o.added_rules.is_empty() && o.brought_back.is_none());

    // The tree comment that introduced the line goes with it.
    let (_, o) = include_row(&["# Tree: excluded /s/a/c.png", "/s/a/c.png"], include("/s/a/c.png"), &[]);
    assert!(o.excludes.is_empty(), "{:?}", o.excludes);

    // Every copy of the line goes (newest first, until it is included); nothing else is touched.
    let (_, o) = include_row(&["/s/a/c.png", "*.log", "/s/a/c.png"], include("/s/a/c.png"), &["*.log"]);
    assert_eq!(o.removed_rules, ["/s/a/c.png", "/s/a/c.png"]);
}

#[test]
fn including_a_folder_by_deleting_its_line_leaves_rules_about_its_contents() {
    // F2: deleting the folder's exact line brings it back, but an earlier rule about a file inside
    // it stays in force.
    let (i, o) = include_row(&["/s/d/x.txt", "/s/d"], include("/s/d"), &["/s/d/x.txt"]);
    assert!(!included_after(&i, &o, "/s/d/x.txt"));
    assert!(included_after(&i, &o, "/s/d/y.txt"));
}

#[test]
fn an_exact_line_and_a_broad_rule_delete_the_dead_line_and_re_include_only_the_item() {
    // F5: deleting the exact line alone changes nothing (`*.txt` still excludes it), so it stays
    // deleted and a `!` re-include of just this item is added.
    let (i, o) = include_row(&["*.txt", "/s/a/f.txt"], include("/s/a/f.txt"), &["*.txt", "!/s/a/f.txt"]);
    assert_eq!(o.removed_rules, ["/s/a/f.txt"]);
    assert_eq!(o.added_rules, ["!/s/a/f.txt"]);
    assert!(!included_after(&i, &o, "/s/a/g.txt"));
}

#[test]
fn an_item_excluded_by_a_broad_rule_only_is_re_included_alone() {
    let (i, o) = include_row(&["*.tmp"], include("/s/t.tmp"), &["*.tmp", "!/s/t.tmp"]);
    assert!(o.removed_rules.is_empty());
    assert!(!included_after(&i, &o, "/s/u.tmp"));
}

#[test]
fn including_a_folder_excluded_by_name_re_includes_it_whole() {
    let (i, o) = include_row(&["node_modules"], include("/s/a/node_modules"), &["node_modules", "!/s/a/node_modules"]);
    assert_eq!(o.added_rules, ["!/s/a/node_modules"]);
    assert!(included_after(&i, &o, "/s/a/node_modules/x.js"));
    assert!(!included_after(&i, &o, "/s/c/deep/node_modules"));
}

#[test]
fn carve_outs_from_one_excluded_folder_start_at_the_highest_excluded_node() {
    let chain = [
        "!/s/a/node_modules",
        "/s/a/node_modules/*",
        "!/s/a/node_modules/lib",
        "/s/a/node_modules/lib/*",
        "!/s/a/node_modules/lib/index.js",
    ];
    let mut after: Vec<&str> = vec!["node_modules"];
    after.extend(chain);
    let (i, o) = include_row(&["node_modules"], include("/s/a/node_modules/lib/index.js"), &after);
    assert_eq!(o.added_rules, chain);
    // Everything else in there is still out, and so is every other node_modules.
    assert!(!included_after(&i, &o, "/s/a/node_modules/lib/other.js"));
    assert!(!included_after(&i, &o, "/s/a/node_modules/other"));
    assert!(!included_after(&i, &o, "/s/c/node_modules/lib/index.js"));

    // A sibling of the first carve-out takes one rule (F3: re-adding the whole chain would drop
    // index.js again).
    let i = input(&["/s"], &o.excludes.iter().map(String::as_str).collect::<Vec<_>>());
    let o = edit(&i, &include("/s/a/node_modules/lib/other.js")).unwrap();
    after.push("!/s/a/node_modules/lib/other.js");
    assert_eq!(patterns(&o.excludes), after);
    assert_eq!(o.added_rules, ["!/s/a/node_modules/lib/other.js"]);
    assert_tree_comment(&o, "/s/a/node_modules/lib/other.js");
    assert!(included_after(&i, &o, "/s/a/node_modules/lib/other.js"));
    assert!(included_after(&i, &o, "/s/a/node_modules/lib/index.js"));

    // An item in another folder of the carved-out one: the chain starts at that folder.
    let i = input(&["/s"], &o.excludes.iter().map(String::as_str).collect::<Vec<_>>());
    let o = edit(&i, &include("/s/a/node_modules/other/z.js")).unwrap();
    let more = ["!/s/a/node_modules/other", "/s/a/node_modules/other/*", "!/s/a/node_modules/other/z.js"];
    after.extend(more);
    assert_eq!(patterns(&o.excludes), after);
    assert_eq!(o.added_rules, more);
    assert_tree_comment(&o, "/s/a/node_modules/other/z.js");
    for p in ["/s/a/node_modules/other/z.js", "/s/a/node_modules/lib/other.js", "/s/a/node_modules/lib/index.js"] {
        assert!(included_after(&i, &o, p), "{p}");
    }
    assert!(!included_after(&i, &o, "/s/a/node_modules/other/y.js"));
    assert!(!included_after(&i, &o, "/s/a/node_modules/third"));
}

#[test]
fn an_item_inside_an_excluded_folder_keeps_its_own_exact_line_and_is_carved_out() {
    let (i, o) = include_row(
        &["/s/X", "/s/X/a.txt"],
        include("/s/X/a.txt"),
        &["/s/X", "/s/X/a.txt", "!/s/X", "/s/X/*", "!/s/X/a.txt"],
    );
    assert!(o.removed_rules.is_empty());
    assert!(!included_after(&i, &o, "/s/X/b.txt"));
}

#[test]
fn include_everything_adds_a_re_include_and_keeps_the_exact_lines() {
    // F1: `!/s/d` brings back the folder and everything inside it, x.txt included.
    let (i, o) = include_row(&["/s/d/x.txt", "/s/d"], include_everything("/s/d"), &["/s/d/x.txt", "/s/d", "!/s/d"]);
    assert!(included_after(&i, &o, "/s/d/x.txt"));
    assert!(o.removed_rules.is_empty());

    // On a folder that is itself included (something inside it isn't): no "already included".
    let (i, o) = include_row(&["/s/d/x.txt"], include_everything("/s/d"), &["/s/d/x.txt", "!/s/d"]);
    assert!(included_after(&i, &o, "/s/d/x.txt"));
}

#[test]
fn include_everything_is_refused_outside_every_source_and_where_rules_cannot_reach() {
    let err = edit(&input(&["/s"], &[]), &include_everything("/elsewhere")).unwrap_err();
    assert_eq!(err, "It isn't inside a backup source.");

    // `!/s/X/d` can't help while /s/X itself is excluded: nothing is returned the engine denies.
    let err = edit(&input(&["/s"], &["/s/X/d", "/s/X"]), &include_everything("/s/X/d")).unwrap_err();
    assert!(err.contains("`/s/X`"), "{err}");
}

#[test]
fn including_a_source_whose_own_path_matches_a_name_pattern_re_includes_its_contents() {
    // The source root is exempt from the patterns, but `node_modules` matches every path inside
    // it, so nothing in the folder would be backed up until it is re-included.
    let i = input(&["/s/a/node_modules"], &["node_modules"]);
    let o = edit(&i, &include("/s/a/node_modules")).unwrap();
    assert_eq!(o.added_rules, ["!/s/a/node_modules"]);
    assert_tree_comment(&o, "/s/a/node_modules");
    assert!(included_after(&i, &o, "/s/a/node_modules/x.js"));
}

#[test]
fn the_tree_comment_says_whether_a_folder_or_a_file_comes_back() {
    // Whether the item is a folder is read from the disk, so this needs real ones.
    let base = scratch("note");
    std::fs::create_dir_all(base.join("kept_dir")).unwrap();
    std::fs::write(base.join("f.tmp"), "x").unwrap();
    let root = canon(base.to_str().unwrap());
    let i = TreeRulesInput { style: PathStyle::NATIVE, ..input(&[&root], &["kept_dir", "*.tmp"]) };
    let comment = |o: &EditOutcome| o.excludes[o.excludes.len() - 2].clone();

    let o = edit(&i, &include(&format!("{root}/kept_dir"))).unwrap();
    assert!(comment(&o).ends_with("and everything inside it"), "{}", comment(&o));
    let o = edit(&i, &include(&format!("{root}/f.tmp"))).unwrap();
    assert!(comment(&o).ends_with("only this item"), "{}", comment(&o));
    let o = edit(&i, &include_everything(&format!("{root}/kept_dir"))).unwrap();
    assert!(comment(&o).ends_with("that other rules exclude"), "{}", comment(&o));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_action_still_carrying_the_old_scope_field_is_read_without_it() {
    let a: EditAction = serde_json::from_str(r#"{"kind":"include","path":"/s/a","scope":"item"}"#).unwrap();
    assert_eq!((a.kind.as_str(), a.path.as_str(), a.pattern), ("include", "/s/a", None));
}

#[test]
fn already_included_things_are_refused() {
    let i = input(&["/s"], &["node_modules"]);
    assert!(edit(&i, &include("/s/a")).unwrap_err().contains("already included"));
    assert!(edit(&i, &include("/s")).unwrap_err().contains("already included"));
}

#[test]
fn an_exclude_file_pattern_that_would_re_exclude_it_is_refused_with_its_line() {
    // The flag excludes /s/a; a file (applied after the flags) excludes the very file asked for,
    // so a `!` added to the plan's own patterns could never win.
    let mut i = input(&["/s"], &["/s/a"]);
    i.exclude_files = vec![crate::commands::cache::ExcludeFile { path: "/rules.txt".into(), ignore_case: false }];
    let read = |p: &str| {
        if p == "/rules.txt" {
            Ok("# keep out\n/s/a/keep.txt\n".to_string())
        } else {
            Err(std::io::Error::other("missing"))
        }
    };
    let err = apply_edit(&i, &include("/s/a/keep.txt"), &MockProbe::default(), &read, &no_env).unwrap_err();
    assert!(err.contains("line 2 of /rules.txt"), "{err}");
    assert!(err.contains("applied after the plan's own patterns"), "{err}");
}

#[test]
fn a_case_insensitive_file_rule_cannot_be_cancelled() {
    let mut i = input(&["/s"], &[]);
    i.exclude_files = vec![crate::commands::cache::ExcludeFile { path: "/ci.txt".into(), ignore_case: true }];
    let read = |_: &str| Ok("Thumbs.db\n".to_string());
    let err = apply_edit(&i, &include("/s/x/thumbs.db"), &MockProbe::default(), &read, &no_env).unwrap_err();
    assert!(err.contains("case-insensitive") || err.contains("line 1"), "{err}");
}

#[test]
fn a_marker_file_exclusion_cannot_be_overridden_by_patterns() {
    let mut i = input(&["/s"], &[]);
    i.exclude_if_present = vec![".nobackup".into()];
    let mut probe = MockProbe::default();
    probe.markers.insert(("/s/b".into(), ".nobackup".into()));
    let err = apply_edit(&i, &include("/s/b/data.bin"), &probe, &no_read, &no_env).unwrap_err();
    assert!(err.contains(".nobackup"), "{err}");
}

// ── exclude an item (a click on an included one) ────────────────────────────

const NM: &str = "/s/a/node_modules";
const INDEX: &str = "/s/a/node_modules/lib/index.js";
const OTHER: &str = "/s/a/node_modules/lib/other.js";

/// The plan with source `/s` and the pattern `node_modules`, with `paths` carved out of
/// /s/a/node_modules by include clicks, in that order.
fn carved(paths: &[&str]) -> TreeRulesInput {
    edits(&input(&["/s"], &["node_modules"]), &paths.iter().map(|p| include(p)).collect::<Vec<_>>())
}

/// One excludeItem click on `i`: the patterns after it (comments aside) are `after`, the clicked
/// path is excluded, and the comments are in place.
fn exclude_row_on(i: &TreeRulesInput, path: &str, after: &[&str]) -> EditOutcome {
    let o = edit(i, &exclude_item(path)).unwrap_or_else(|e| panic!("excludeItem {path} on {:?}: {e}", i.excludes));
    assert_eq!(patterns(&o.excludes), after, "excludeItem {path} on {:?}", i.excludes);
    assert!(!included_after(i, &o, path), "excludeItem {path} on {:?}: {:?}", i.excludes, o.excludes);
    assert_tree_comment(&o, path);
    o
}

/// [`exclude_row_on`] a plan with source `/s` and `before` as its patterns.
fn exclude_row(before: &[&str], path: &str, after: &[&str]) -> (TreeRulesInput, EditOutcome) {
    let i = input(&["/s"], before);
    let o = exclude_row_on(&i, path, after);
    (i, o)
}

#[test]
fn excluding_an_included_item_appends_its_exact_rule() {
    let (i, o) = exclude_row(&["*.log"], "/s/a/c-1.png", &["*.log", "/s/a/c-1.png"]);
    assert_eq!(o.excludes, ["*.log", "# Tree: excluded /s/a/c-1.png", "/s/a/c-1.png"]);
    assert_eq!(o.added_rules, ["/s/a/c-1.png"]);
    assert!(o.removed_rules.is_empty() && o.brought_back.is_none());
    assert!(included_after(&i, &o, "/s/a/c-2.png"));

    // A name with glob characters is written escaped, so it excludes only itself.
    let (i, o) = exclude_row(&[], "/s/we[ird]*", &["/s/we\\[ird\\]\\*"]);
    assert!(included_after(&i, &o, "/s/wei"));
}

#[test]
fn excluding_an_item_a_lone_re_include_brings_back_deletes_that_re_include() {
    // The tree's own re-include goes, and its comment with it.
    let (_, o) = exclude_row(&["*.tmp", "# Tree: included /s/t.tmp — why", "!/s/t.tmp"], "/s/t.tmp", &["*.tmp"]);
    assert_eq!(o.excludes, ["*.tmp"]);
    assert_eq!(o.removed_rules, ["!/s/t.tmp"]);
    assert!(o.added_rules.is_empty());

    // A legacy exact-line-and-re-include pair loses the re-include.
    exclude_row(&["/s/a/f.txt", "!/s/a/f.txt"], "/s/a/f.txt", &["/s/a/f.txt"]);

    // Every copy goes, newest first, until it is excluded…
    let (_, o) = exclude_row(&["*.tmp", "!/s/t.tmp", "!/s/t.tmp"], "/s/t.tmp", &["*.tmp"]);
    assert_eq!(o.removed_rules, ["!/s/t.tmp", "!/s/t.tmp"]);
    // …and no further: here deleting the newest one is enough.
    exclude_row(&["*.tmp", "!/s/t.tmp", "/s/t.tmp", "!/s/t.tmp"], "/s/t.tmp", &["*.tmp", "!/s/t.tmp", "/s/t.tmp"]);

    // A comment the user wrote is never taken.
    let (_, o) = exclude_row(&["# my note", "*.tmp", "!/s/t.tmp"], "/s/t.tmp", &["*.tmp"]);
    assert_eq!(o.excludes, ["# my note", "*.tmp"]);
}

#[test]
fn excluding_the_only_carved_out_item_undoes_the_carve_out_and_its_scaffolding() {
    let o = exclude_row_on(&carved(&[INDEX]), INDEX, &["node_modules"]);
    // No tree comment is left (it sat above the scaffolding), and no empty node_modules either (F4).
    assert_eq!(o.excludes, ["node_modules"]);
    assert_eq!(
        o.removed_rules,
        [
            "!/s/a/node_modules/lib/index.js",
            "!/s/a/node_modules",
            "/s/a/node_modules/*",
            "!/s/a/node_modules/lib",
            "/s/a/node_modules/lib/*",
        ]
    );
}

#[test]
fn scaffolding_another_carve_out_still_uses_stays() {
    let i = carved(&[INDEX, OTHER]);
    let o = edit(&i, &exclude_item(INDEX)).unwrap();
    assert_eq!(
        patterns(&o.excludes),
        [
            "node_modules",
            "!/s/a/node_modules",
            "/s/a/node_modules/*",
            "!/s/a/node_modules/lib",
            "/s/a/node_modules/lib/*",
            "!/s/a/node_modules/lib/other.js",
        ]
    );
    assert_eq!(o.removed_rules, ["!/s/a/node_modules/lib/index.js"]);
    assert!(o.added_rules.is_empty());
    assert!(!included_after(&i, &o, INDEX));
    assert!(included_after(&i, &o, OTHER));

    // Undoing the other one as well takes the scaffolding with it. The comment written for
    // index.js sits above that scaffolding and names index.js, not other.js, so it is left behind
    // (harmless: a comment changes nothing).
    let o = edit(&applied(&i, &o), &exclude_item(OTHER)).unwrap();
    assert_eq!(patterns(&o.excludes), ["node_modules"]);
    assert!(o.excludes[1].starts_with("# Tree: included /s/a/node_modules/lib/index.js"), "{:?}", o.excludes);
}

#[test]
fn undoing_carve_outs_newest_first_returns_the_plan_exactly() {
    let one = carved(&[INDEX]);
    let after_other = edits(&carved(&[INDEX, OTHER]), &[exclude_item(OTHER)]);
    assert_eq!(after_other.excludes, one.excludes);
    let after_both = edits(&after_other, &[exclude_item(INDEX)]);
    assert_eq!(after_both.excludes, ["node_modules"]);
}

#[test]
fn excluding_a_partly_carved_folder_appends_its_exact_rule_and_keeps_the_scaffolding() {
    // node_modules is included only through the carve-out's `!/s/a/node_modules`, which is
    // scaffolding (index.js hangs off it), so that line is never deleted by a click on the folder.
    let i = carved(&[INDEX]);
    let mut after = patterns(&i.excludes);
    after.push(NM);
    let o = exclude_row_on(&i, NM, &after);
    assert!(o.removed_rules.is_empty());
    assert!(!included_after(&i, &o, INDEX));

    // Including it again deletes that line: back to exactly the carve-out.
    let back = edit(&applied(&i, &o), &include(NM)).unwrap();
    assert_eq!(back.excludes, i.excludes);
}

#[test]
fn a_re_include_that_is_not_what_includes_the_item_stays_and_its_exact_rule_is_appended() {
    // `!/s/d` brought back x.txt, but /s/d itself was included anyway: deleting it would leave /s/d
    // included, so it stays and the folder's exact rule is appended after it.
    let i = edits(&input(&["/s"], &["/s/d/x.txt"]), &[include_everything("/s/d")]);
    let o = exclude_row_on(&i, "/s/d", &["/s/d/x.txt", "!/s/d", "/s/d"]);
    assert_eq!(o.excludes[1..3], i.excludes[1..3], "the re-include keeps its own comment");
    assert!(o.removed_rules.is_empty());
    assert_eq!(o.added_rules, ["/s/d"]);

    // The same for a re-include nothing excludes in the first place.
    exclude_row(&["!/s/t.tmp"], "/s/t.tmp", &["!/s/t.tmp", "/s/t.tmp"]);
}

#[test]
fn scaffolding_stays_when_the_folder_it_opened_would_not_be_excluded_without_it() {
    // Nothing else excludes /s/X: `!/s/X`, `/s/X/*` is what keeps the rest of X out, so undoing
    // a's carve-out leaves it.
    let (i, o) = exclude_row(&["!/s/X", "/s/X/*", "!/s/X/a"], "/s/X/a", &["!/s/X", "/s/X/*"]);
    assert!(!included_after(&i, &o, "/s/X/b"));
    assert_eq!(o.removed_rules, ["!/s/X/a"]);

    // Here only a's own line would exclude it without the scaffolding, and /s/X/b would come back.
    let (i, o) = exclude_row(&["/s/X/a", "!/s/X", "/s/X/*", "!/s/X/a"], "/s/X/a", &["/s/X/a", "!/s/X", "/s/X/*"]);
    assert!(!included_after(&i, &o, "/s/X/b"));
}

#[test]
fn excluding_an_item_the_tree_just_included_returns_the_plan_to_how_it_was() {
    let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // The rows of the include table whose click only appended rules.
    let rows: Vec<(Vec<String>, EditAction)> = vec![
        (lines(&["*.tmp"]), include("/s/t.tmp")),
        (lines(&["node_modules"]), include(NM)),
        (lines(&["node_modules"]), include(INDEX)),
        (carved(&[INDEX]).excludes, include(OTHER)),
        (carved(&[INDEX, OTHER]).excludes, include("/s/a/node_modules/other/z.js")),
        (lines(&["/s/X", "/s/X/a.txt"]), include("/s/X/a.txt")),
        (lines(&["/s/d/x.txt", "/s/d"]), include_everything("/s/d")),
    ];
    for (before, action) in rows {
        let i = input(&["/s"], &before.iter().map(String::as_str).collect::<Vec<_>>());
        let there = edit(&i, &action).unwrap();
        assert!(!there.added_rules.is_empty() && there.removed_rules.is_empty(), "{} {}", action.kind, action.path);
        let back = edit(&applied(&i, &there), &exclude_item(&action.path)).unwrap();
        assert_eq!(back.excludes, before, "{} {} then excludeItem", action.kind, action.path);
        assert_eq!(back.sources, i.sources);
        let (mut added, mut removed) = (there.added_rules.clone(), back.removed_rules.clone());
        added.sort();
        removed.sort();
        assert_eq!(removed, added, "the undo deletes exactly what the click added");
    }

    // Re-includes written before the carve-out (a dead hand-written one; an older carve-out out of
    // the folder before the folder itself was excluded) don't use its scaffolding, so it goes too.
    let older = edits(&carved(&[INDEX, OTHER]), &[exclude_item(NM)]);
    for (start, p) in [(input(&["/s"], &["!/s/X/a", "/s/X"]), "/s/X/a"), (older, INDEX)] {
        let there = edit(&start, &include(p)).unwrap();
        assert!(there.removed_rules.is_empty(), "{:?}", there);
        let back = edit(&applied(&start, &there), &exclude_item(p)).unwrap();
        assert_eq!(back.excludes, start.excludes, "include {p} then excludeItem");
    }

    // Including f.txt here also deleted a line that did nothing (`*.txt` excludes it anyway), and
    // the undo can't know that line was there: it comes back to the same included set, not to
    // the same list.
    let i = input(&["/s"], &["*.txt", "/s/a/f.txt"]);
    let there = edit(&i, &include("/s/a/f.txt")).unwrap();
    let back = edit(&applied(&i, &there), &exclude_item("/s/a/f.txt")).unwrap();
    assert_eq!(back.excludes, ["*.txt"]);
    for p in ["/s/a/f.txt", "/s/a/g.txt", "/s/a", "/s/b.png"] {
        assert_eq!(included_after(&i, &back, p), included(&i, p), "{p}");
    }
}

#[test]
fn excluding_a_source_removes_it_and_excluded_things_are_refused() {
    let i = input(&["/s", "/t"], &["*.tmp"]);
    let o = edit(&i, &exclude_item("/t")).unwrap();
    assert_eq!(o.sources, ["/s"]);
    assert_eq!(o.removed_sources, ["/t"]);
    assert!(o.added_rules.is_empty() && o.removed_rules.is_empty());
    assert_eq!(o.excludes, ["*.tmp"]);

    assert_eq!(edit(&i, &exclude_item("/s/x.tmp")).unwrap_err(), "It's already excluded.");
    assert!(edit(&i, &exclude_item("/")).unwrap_err().contains("isn't backed up itself"));

    // A source from a path-list file can't be excluded by a pattern.
    let mut i = input(&["/s"], &[]);
    i.list_sources = vec!["/l/notes.txt".into()];
    assert!(edit(&i, &exclude_item("/l/notes.txt")).unwrap_err().contains("path-list file"));
}

// ── export ──────────────────────────────────────────────────────────────────

#[test]
fn export_writes_ordered_files_markers_and_quoted_sources() {
    let mut i = input(&["/my docs", "/b'c"], &["node_modules", "/s/cost$5", "!/s/keep"]);
    i.exclude_if_present = vec![".nobackup".into(), "CACHEDIR.TAG:Signature: 8a".into()];
    i.exclude_caches = true;
    i.exclude_files = vec![crate::commands::cache::ExcludeFile { path: "/ci.txt".into(), ignore_case: true }];
    let read = |_: &str| Ok("THUMBS.DB\n".to_string());
    let built = build_rules(&i, &read, &no_env);
    let out = export_rules(&built.rules);

    let lines: Vec<&str> = out.exclude_file.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines, ["node_modules", "/s/cost$$5", "!/s/keep"]);
    assert_eq!(out.iexclude_file.as_deref().map(|f| f.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>()), Some(vec!["THUMBS.DB"]));
    assert_eq!(
        out.command,
        "restic backup --exclude-file restic-excludes.txt --iexclude-file restic-iexcludes.txt \
         --exclude-if-present .nobackup --exclude-if-present 'CACHEDIR.TAG:Signature: 8a' --exclude-caches \
         '/my docs' '/b'\\''c'"
    );
}

#[test]
fn export_without_case_insensitive_rules_has_one_file() {
    let out = export_rules(&build_rules(&input(&["/s"], &["*.log"]), &no_read, &no_env).rules);
    assert!(out.iexclude_file.is_none() && out.iexclude_file_name.is_none());
    assert_eq!(out.command, "restic backup --exclude-file restic-excludes.txt /s");
    let none = export_rules(&build_rules(&input(&["/s"], &[]), &no_read, &no_env).rules);
    assert_eq!(none.command, "restic backup /s");
}

#[test]
fn an_exported_file_reads_back_to_the_same_rules() {
    // rules → file text → read like restic reads it → the same ordered patterns.
    let i = input(&["/s"], &["node_modules", "/s/cost$5", "!/s/we\\[ird\\]", "dir with spaces/*.tmp"]);
    let built = build_rules(&i, &no_read, &no_env);
    let out = export_rules(&built.rules);

    let mut back = input(&["/s"], &[]);
    back.exclude_files = vec![crate::commands::cache::ExcludeFile { path: "/exported.txt".into(), ignore_case: false }];
    let text = out.exclude_file.clone();
    let read = move |_: &str| Ok(text.clone());
    let rebuilt = build_rules(&back, &read, &no_env);
    let a: Vec<&str> = built.rules.rules.iter().map(|r| r.pattern.as_str()).collect();
    let b: Vec<&str> = rebuilt.rules.rules.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(a, b);
}

// ── Windows paths (pure string logic, so it runs on every platform) ─────────

fn win_input(sources: &[&str], excludes: &[&str]) -> TreeRulesInput {
    TreeRulesInput { style: PathStyle::Windows, ..input(sources, excludes) }
}

#[test]
fn windows_edits_keep_sources_native_and_patterns_in_slash_form() {
    let i = win_input(&["C:\\Users\\me"], &[]);
    let o = edit(&i, &exclude("C:\\Users\\me\\docs\\old", "C:/Users/me/docs/old")).unwrap();
    // The plan keeps the OS spelling for sources; patterns use the form restic accepts on Windows.
    assert_eq!(o.sources, ["C:\\Users\\me"]);
    assert_eq!(o.added_rules, ["C:/Users/me/docs/old"]);
    assert!(!included_after(&i, &o, "C:\\Users\\me\\docs\\old\\x.txt"));
    assert!(included_after(&i, &o, "C:\\Users\\me\\docs\\new\\x.txt"));
}

#[test]
fn windows_excludes_a_folder_with_a_bracket_using_a_character_class() {
    let i = win_input(&["C:\\s"], &[]);
    let path = "C:/s/we[ird]";
    let o = edit(&i, &exclude(path, &PathStyle::Windows.escape_glob(path))).unwrap();
    assert_eq!(o.added_rules, ["C:/s/we[[]ird]"]);
    assert_eq!(o.excludes, ["# Tree: excluded C:/s/we[ird] — with pattern C:/s/we[[]ird]", "C:/s/we[[]ird]"]);
    assert!(!included_after(&i, &o, "C:/s/we[ird]/inner"));
    assert!(included_after(&i, &o, "C:/s/weird"));
}

#[test]
fn windows_including_inside_an_excluded_folder_carves_it_out() {
    let i = win_input(&["C:\\Users\\me\\app"], &["node_modules"]);
    let o = edit(&i, &include("C:\\Users\\me\\app\\node_modules\\lib\\index.js")).unwrap();
    assert_eq!(
        o.added_rules,
        [
            "!C:/Users/me/app/node_modules",
            "C:/Users/me/app/node_modules/*",
            "!C:/Users/me/app/node_modules/lib",
            "C:/Users/me/app/node_modules/lib/*",
            "!C:/Users/me/app/node_modules/lib/index.js",
        ]
    );
    assert!(included_after(&i, &o, "C:/Users/me/app/node_modules/lib/index.js"));
    assert!(!included_after(&i, &o, "C:/Users/me/app/node_modules/lib/other.js"));
    assert_tree_comment(&o, "C:/Users/me/app/node_modules/lib/index.js");

    // Excluding it again undoes the carve-out, scaffolding and comment included.
    let back = edit(&applied(&i, &o), &exclude_item("C:\\Users\\me\\app\\node_modules\\lib\\index.js")).unwrap();
    assert_eq!(back.excludes, ["node_modules"]);
    assert_eq!(back.sources, ["C:\\Users\\me\\app"]);
}

#[test]
fn windows_excludes_an_item_by_its_exact_rule_in_the_escaped_spelling() {
    let i = win_input(&["C:\\s"], &[]);
    let o = edit(&i, &exclude_item("C:\\s\\we[ird]\\f.txt")).unwrap();
    assert_eq!(o.excludes, ["# Tree: excluded C:/s/we[ird]/f.txt", "C:/s/we[[]ird]/f.txt"]);
    assert_eq!(o.added_rules, ["C:/s/we[[]ird]/f.txt"]);
    assert_tree_comment(&o, "C:/s/we[ird]/f.txt");
    assert!(!included_after(&i, &o, "C:/s/we[ird]/f.txt"));
    // The look-alike an unescaped `we[ird]` would match stays included.
    assert!(included_after(&i, &o, "C:/s/wei/f.txt"));

    // A re-include in that spelling is deleted instead, whichever way the clicked path is spelled.
    let i = win_input(&["C:\\s"], &["*.txt"]);
    let there = edit(&i, &include("C:/s/we[ird]/f.txt")).unwrap();
    let back = edit(&applied(&i, &there), &exclude_item("C:\\s\\we[ird]\\f.txt")).unwrap();
    assert_eq!(back.excludes, ["*.txt"]);
    assert_eq!(back.removed_rules, ["!C:/s/we[[]ird]/f.txt"]);
    assert_tree_comment(&back, "C:/s/we[ird]/f.txt");
}

#[test]
fn windows_re_includes_and_deletes_exact_lines_in_the_escaped_spelling() {
    // Excluded by a broad rule: a `!` re-include with the bracket written as a character class.
    let i = win_input(&["C:/s"], &["*.txt"]);
    let o = edit(&i, &include("C:/s/we[ird]/f.txt")).unwrap();
    assert_eq!(patterns(&o.excludes), ["*.txt", "!C:/s/we[[]ird]/f.txt"]);
    assert_tree_comment(&o, "C:/s/we[ird]/f.txt");
    assert!(included_after(&i, &o, "C:/s/we[ird]/f.txt"));
    assert!(!included_after(&i, &o, "C:/s/weird/f.txt"));

    // Excluded by its own exact line in that spelling: the line is deleted, whichever way the
    // clicked path is spelled.
    let i = win_input(&["C:\\s"], &["C:/s/we[[]ird]/f.txt"]);
    let o = edit(&i, &include("C:\\s\\we[ird]\\f.txt")).unwrap();
    assert!(o.excludes.is_empty(), "{:?}", o.excludes);
    assert_eq!(o.removed_rules, ["C:/s/we[[]ird]/f.txt"]);
    assert_tree_comment(&o, "C:/s/we[ird]/f.txt");
}

#[test]
fn windows_include_everything_re_includes_the_folder_in_the_escaped_spelling() {
    // A partly included folder: `!` plus the bracket as a character class brings back what is
    // inside it; the exact lines stay, and the look-alike `wei` (which an unescaped `we[ird]`
    // would match) keeps its own exclusion.
    let i = win_input(&["C:\\s"], &["C:/s/we[[]ird]/x.txt", "C:/s/wei/x.txt"]);
    let o = edit(&i, &include_everything("C:\\s\\we[ird]")).unwrap();
    assert_eq!(patterns(&o.excludes), ["C:/s/we[[]ird]/x.txt", "C:/s/wei/x.txt", "!C:/s/we[[]ird]"]);
    assert_eq!(o.added_rules, ["!C:/s/we[[]ird]"]);
    assert!(o.removed_rules.is_empty());
    assert_tree_comment(&o, "C:/s/we[ird]");
    assert!(included_after(&i, &o, "C:/s/we[ird]"));
    assert!(included_after(&i, &o, "C:/s/we[ird]/x.txt"));
    assert!(!included_after(&i, &o, "C:/s/wei/x.txt"));
}

#[test]
fn windows_adds_and_removes_sources_in_the_native_spelling() {
    let i = win_input(&["D:\\data\\a", "D:\\data\\b"], &[]);
    let o = edit(&i, &include("D:\\data")).unwrap();
    assert_eq!(o.added_sources, ["D:\\data"]);
    assert_eq!(o.removed_sources, ["D:\\data\\a", "D:\\data\\b"]);
    assert_eq!(o.sources, ["D:\\data"]);

    let o = edit(&win_input(&["C:\\s", "C:\\t"], &[]), &exclude("C:\\t", "C:/t")).unwrap();
    assert_eq!(o.removed_sources, ["C:\\t"]);
    assert_eq!(o.sources, ["C:\\s"]);
}

#[test]
fn windows_export_quotes_for_powershell_and_writes_slash_patterns() {
    let mut i = win_input(&["C:\\Program Files\\App", "D:\\data"], &["node_modules", "C:/s/cost$5"]);
    i.exclude_if_present = vec![".nobackup".into()];
    let built = build_rules(&i, &no_read, &no_env);
    let out = export_rules(&built.rules);
    assert_eq!(
        out.command,
        "restic backup --exclude-file restic-excludes.txt --exclude-if-present .nobackup \
         'C:\\Program Files\\App' D:\\data"
    );
    assert!(out.notes.iter().any(|n| n.contains("PowerShell")));
    let lines: Vec<&str> = out.exclude_file.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines, ["node_modules", "C:/s/cost$$5"]);
}

#[test]
fn windows_export_never_lets_powershell_expand_or_split_a_name() {
    // `$(…)`, `$var`, a backtick, `%VAR%`, `,` and a leading `@` all do something in a shell; inside
    // PowerShell single quotes none of them does, and a `'` is doubled.
    let names = [
        "C:\\x\\$(calc)",
        "C:\\x\\$env:USERNAME",
        "C:\\x\\a`b",
        "C:\\x\\%TEMP%",
        "C:\\x\\a,b",
        "C:\\x\\it's",
        // PowerShell also reads the typographic single quotes as quote characters.
        "C:\\x\\it\u{2019}s; calc",
        "C:\\x\\\u{2018}a\u{201A}b\u{201B}",
    ];
    let i = win_input(&names, &[]);
    let out = export_rules(&build_rules(&i, &no_read, &no_env).rules);
    assert_eq!(
        out.command,
        "restic backup 'C:\\x\\$(calc)' 'C:\\x\\$env:USERNAME' 'C:\\x\\a`b' 'C:\\x\\%TEMP%' 'C:\\x\\a,b' 'C:\\x\\it''s' \
         'C:\\x\\it\u{2019}\u{2019}s; calc' 'C:\\x\\\u{2018}\u{2018}a\u{201A}\u{201A}b\u{201B}\u{201B}'"
    );
}

#[test]
fn an_argument_no_shell_can_carry_safely_leaves_the_command_out() {
    // A double quote (hand-typed marker, Windows) or a control character (any OS): no command,
    // with a note naming the culprit — but the exclude file is still produced.
    let mut i = win_input(&["C:\\s"], &["node_modules"]);
    i.exclude_if_present = vec!["we\"ird".into()];
    let out = export_rules(&build_rules(&i, &no_read, &no_env).rules);
    assert_eq!(out.command, "");
    assert!(out.notes.iter().any(|n| n.contains("No command is shown") && n.contains("we\\\"ird")));
    assert!(out.exclude_file.contains("node_modules"));

    // The Unicode line/paragraph separators count too.
    let out = export_rules(&build_rules(&win_input(&["C:\\a\u{2028}b"], &[]), &no_read, &no_env).rules);
    assert_eq!(out.command, "");

    let out = export_rules(&build_rules(&input(&["/a\nb"], &[]), &no_read, &no_env).rules);
    assert_eq!(out.command, "");
    assert!(out.notes.iter().any(|n| n.contains("No command is shown") && n.contains("/a\\nb")));

    // The Unix quote handles a `"` fine.
    let mut u = input(&["/s"], &[]);
    u.exclude_if_present = vec!["we\"ird".into()];
    let out = export_rules(&build_rules(&u, &no_read, &no_env).rules);
    assert_eq!(out.command, "restic backup --exclude-if-present 'we\"ird' /s");
}

#[test]
fn unix_quoting_still_single_quotes_a_path_with_a_backslash() {
    // A backslash is the shell's escape on Unix, so it must be quoted there.
    let i = input(&["/a/b\\c d"], &[]);
    let out = export_rules(&build_rules(&i, &no_read, &no_env).rules);
    assert_eq!(out.command, "restic backup '/a/b\\c d'");
}

// ── collateral and "brought back" (real folders) ────────────────────────────

/// A real folder tree in a scratch dir, removed when dropped. `root` and `at()` are in the `/`
/// spelling the engine uses.
struct Tree {
    root: String,
}

impl Tree {
    /// `files` are relative paths (`/`-separated, parents created); one ending in `/` is an
    /// empty folder.
    fn new(label: &str, files: &[&str]) -> Tree {
        let root = scratch(label);
        for f in files {
            let p = root.join(f);
            if f.ends_with('/') {
                std::fs::create_dir_all(&p).unwrap();
            } else {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "x").unwrap();
            }
        }
        Tree { root: canon(root.to_str().unwrap()) }
    }

    fn at(&self, rel: &str) -> String {
        format!("{}/{rel}", self.root)
    }

    /// `at(rel)` as a pattern.
    fn rule(&self, rel: &str) -> String {
        esc(&self.at(rel))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(PathStyle::NATIVE.to_native(&self.root));
    }
}

/// A plan over real paths: the platform's own spelling, unlike `input`.
fn real_input(sources: &[&str], excludes: &[String]) -> TreeRulesInput {
    TreeRulesInput {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        excludes: excludes.to_vec(),
        ..Default::default()
    }
}

fn engine_of(i: &TreeRulesInput) -> Engine {
    build_engine(i, &no_read, &no_env).unwrap().0
}

fn lines_of(rules: &[&str]) -> Vec<String> {
    rules.iter().map(|r| r.to_string()).collect()
}

#[test]
fn collateral_finds_a_flipped_sibling() {
    let t = Tree::new("collateral", &["a.txt", "b.txt"]);
    let before = engine_of(&real_input(&[&t.root], &[]));
    let after = engine_of(&real_input(&[&t.root], &[t.rule("b.txt")]));
    let probe = MockProbe::default();
    let find = |target: &str| find_collateral(&before, &after, &probe, target, COLLATERAL_LIMITS);
    assert_eq!(find(&t.at("a.txt")), Some(t.at("b.txt")), "b.txt flipped and the click was on a.txt");
    assert_eq!(find(&t.at("b.txt")), None, "the clicked item itself may flip");
    assert_eq!(find(&t.root), None, "so may everything inside the clicked folder");
}

#[test]
fn collateral_is_looked_for_in_every_source() {
    let t = Tree::new("collateral-sources", &["one/a.txt", "two/b.txt", "two/deep/c.txt"]);
    let sources = [t.at("one"), t.at("two")];
    let srcs: Vec<&str> = sources.iter().map(String::as_str).collect();
    let before = engine_of(&real_input(&srcs, &[]));
    let after = engine_of(&real_input(&srcs, &[t.rule("two/deep/c.txt")]));
    let probe = MockProbe::default();
    assert_eq!(
        find_collateral(&before, &after, &probe, &t.at("one/a.txt"), COLLATERAL_LIMITS),
        Some(t.at("two/deep/c.txt"))
    );
    // A change in the click's own folder is the click's business, wherever that folder is.
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("two"), COLLATERAL_LIMITS), None);
}

#[test]
fn collateral_allows_the_path_down_to_the_clicked_item() {
    // Carving X/sub/c.txt out of the excluded X flips X and X/sub (the way down) and c.txt.
    let t = Tree::new("collateral-path", &["X/sub/c.txt", "X/sub/d.txt", "X/b.txt", "other/e.png"]);
    let before = engine_of(&real_input(&[&t.root], &[t.rule("X")]));
    let chain = carve_out_chain(PathStyle::NATIVE, &t.at("X"), &t.at("X/sub/c.txt"));
    let after = engine_of(&real_input(&[&t.root], &chain));
    let probe = MockProbe::default();
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("X/sub/c.txt"), COLLATERAL_LIMITS), None);
    // Aimed at a different file, the same rules are collateral: X/sub (found first, breadth
    // first) and c.txt came back too. X itself is on the way down to b.txt, so it is allowed.
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("X/b.txt"), COLLATERAL_LIMITS), Some(t.at("X/sub")));
}

#[test]
fn a_capped_collateral_scan_gives_up_without_a_verdict() {
    let t = Tree::new("collateral-capped", &["a.txt", "b.txt", "c.txt", "d.txt"]);
    let before = engine_of(&real_input(&[&t.root], &[]));
    let after = engine_of(&real_input(&[&t.root], &[t.rule("d.txt")]));
    let probe = MockProbe::default();
    // No entries allowed: the very first one trips the cap, whichever order the folder lists them in.
    let tight = ScanLimits { budget: std::time::Duration::from_secs(30), max_entries: 0 };
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("a.txt"), tight), None);
    let roomy = ScanLimits { max_entries: 100, ..tight };
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("a.txt"), roomy), Some(t.at("d.txt")));
}

#[test]
fn the_collateral_budget_covers_a_long_list_of_sources_too() {
    // A path-list file adds one source per line; looking at them is part of the scan.
    let t = Tree::new("collateral-budget", &["a.txt", "b.txt"]);
    let many: Vec<String> = (0..20_000).map(|n| format!("{}-lists/{n}", t.root)).collect();
    let mut sources: Vec<&str> = many.iter().map(String::as_str).collect();
    sources.push(&t.root);
    let rules = [t.rule("b.txt")];
    let (before, after) = (engine_of(&real_input(&sources, &[])), engine_of(&real_input(&sources, &rules)));
    let probe = MockProbe::default();

    let spent = ScanLimits { budget: std::time::Duration::ZERO, max_entries: u64::MAX };
    let started = std::time::Instant::now();
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("a.txt"), spent), None, "an expired budget is no verdict");
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "gave up only after {:?}", started.elapsed());

    // Not simply blind to such plans: with a few sources and time to spare it finds b.txt.
    let few = [t.at("x-lists/1"), t.at("x-lists/2"), t.root.clone()];
    let few: Vec<&str> = few.iter().map(String::as_str).collect();
    let (before, after) = (engine_of(&real_input(&few, &[])), engine_of(&real_input(&few, &rules)));
    let roomy = ScanLimits { budget: std::time::Duration::from_secs(30), max_entries: u64::MAX };
    assert_eq!(find_collateral(&before, &after, &probe, &t.at("a.txt"), roomy), Some(t.at("b.txt")));
}

#[test]
fn a_collateral_hit_refuses_the_edit_with_the_path() {
    let t = Tree::new("collateral-refused", &["a.txt", "b.txt"]);
    let before = real_input(&[&t.root], &[]);
    let probe = MockProbe::default();
    // The rules an edit of a.txt would never write: they also drop b.txt.
    let after = real_input(&[&t.root], &[t.rule("a.txt"), t.rule("b.txt")]);
    let err = refuse_collateral(&before, &after, &t.at("a.txt"), &probe, &no_read, &no_env, COLLATERAL_LIMITS).unwrap_err();
    assert_eq!(err, format!("This change would also change {}, so it wasn't made.", PathStyle::NATIVE.to_native(&t.at("b.txt"))));
    let ok = real_input(&[&t.root], &[t.rule("a.txt")]);
    assert_eq!(refuse_collateral(&before, &ok, &t.at("a.txt"), &probe, &no_read, &no_env, COLLATERAL_LIMITS), Ok(()));
}

#[test]
fn normal_edits_have_no_collateral() {
    let t = Tree::new("collateral-normal", &["X/a.txt", "X/b.txt", "X/sub/c.txt", "other/e.png"]);
    let i = real_input(&[&t.root], &[t.rule("X")]);
    let a = t.at("X/a.txt");
    let c = t.at("X/sub/c.txt");

    let o1 = edit(&i, &include(&a)).expect("including a.txt is not collateral");
    assert!(included_after(&i, &o1, &a));
    let i1 = applied(&i, &o1);

    let o2 = edit(&i1, &include(&c)).expect("including c.txt is not collateral");
    assert!(included_after(&i1, &o2, &c));
    assert!(included_after(&i1, &o2, &a), "a.txt stays included: {:?}", o2.excludes);
    let i2 = applied(&i1, &o2);
    assert!(!included(&i2, &t.at("X/b.txt")), "b.txt was never asked for");
    assert!(included(&i2, &t.at("other/e.png")));

    // Excluding them again undoes the carve-outs the same way, newest first.
    let o3 = edit(&i2, &exclude_item(&c)).expect("excluding c.txt is not collateral");
    assert!(!included_after(&i2, &o3, &c));
    assert!(included_after(&i2, &o3, &a));
}

#[test]
fn an_exclude_pattern_may_change_many_items() {
    // A pattern exists to match more than the clicked item, so it is not collateral.
    let t = Tree::new("collateral-pattern", &["a.txt", "b.txt", "c.md"]);
    let i = real_input(&[&t.root], &[]);
    let o = edit(&i, &exclude(&t.at("a.txt"), "*.txt")).expect("a pattern is not held to the one item");
    assert!(!included_after(&i, &o, &t.at("a.txt")));
    assert!(!included_after(&i, &o, &t.at("b.txt")));
    assert!(included_after(&i, &o, &t.at("c.md")));
    assert_eq!(o.brought_back, None);
}

#[test]
fn brought_back_counts_what_other_rules_hide() {
    let t = Tree::new("brought-back", &["sub/c.txt", "sub/e.tmp", "keep.txt"]);
    let sub = t.at("sub");
    let run = |rules: &[String], action: EditAction| {
        let i = real_input(&[&t.root], rules);
        let o = edit(&i, &action).unwrap_or_else(|e| panic!("{} {} on {rules:?}: {e}", action.kind, action.path));
        assert!(included_after(&i, &o, &action.path));
        o.brought_back
    };

    // `sub` is excluded by a broad rule: `!sub` re-includes it and, with it, e.tmp, which
    // `*.tmp` also excludes.
    let broad = lines_of(&["*.tmp", "sub"]);
    assert_eq!(run(&broad, include(&sub)), Some(BroughtBack { count: 1, capped: false }));

    // Excluded by its own exact line: that line is deleted and nothing else changes, so nothing
    // comes back (e.tmp is still excluded by `*.tmp`).
    let exact = [t.rule("sub")];
    assert_eq!(run(&exact, include(&sub)), None);
    let exact_and_tmp = ["*.tmp".to_string(), t.rule("sub")];
    assert_eq!(run(&exact_and_tmp, include(&sub)), None);

    // Re-including a folder nothing else hides things in.
    assert_eq!(run(&lines_of(&["sub"]), include(&sub)), None);

    // "Include everything inside it" on a partly included folder.
    let partial = [t.rule("sub/c.txt")];
    assert_eq!(run(&partial, include_everything(&sub)), Some(BroughtBack { count: 1, capped: false }));

    // A file has nothing inside it to bring back.
    assert_eq!(run(&lines_of(&["*.tmp"]), include(&t.at("sub/e.tmp"))), None);
}

#[test]
fn brought_back_also_counts_for_a_carve_out_that_ends_in_a_folder() {
    let t = Tree::new("brought-back-chain", &["X/d/f.tmp", "X/d/g.txt", "X/other.txt"]);
    let rules = ["*.tmp".to_string(), t.rule("X")];
    let i = real_input(&[&t.root], &rules);
    let o = edit(&i, &include(&t.at("X/d"))).unwrap();
    assert_eq!(o.added_rules.last().map(String::as_str), Some(format!("!{}", t.rule("X/d")).as_str()));
    assert_eq!(o.brought_back, Some(BroughtBack { count: 1, capped: false }));
    // The same carve-out aimed at a file ends in no folder.
    let o = edit(&i, &include(&t.at("X/d/g.txt"))).unwrap();
    assert_eq!(o.brought_back, None);
}

#[test]
fn count_brought_back_counts_a_rejected_folder_once_and_stops_at_its_cap() {
    let t = Tree::new("brought-back-count", &["sub/junk.tmp/a.tmp", "sub/junk.tmp/b.tmp", "sub/1.tmp", "sub/2.tmp", "sub/ok.txt"]);
    let others = engine_of(&real_input(&[&t.root], &lines_of(&["*.tmp"])));
    let after = engine_of(&real_input(&[&t.root], &["*.tmp".to_string(), format!("!{}", t.rule("sub"))]));
    let probe = MockProbe::default();
    let roomy = ScanLimits { budget: std::time::Duration::from_secs(30), max_entries: 1000 };
    let all = count_brought_back(&others, &after, &probe, &t.at("sub"), roomy);
    assert_eq!((all.count, all.capped), (3, false), "junk.tmp (once), 1.tmp and 2.tmp: {all:?}");
    assert!(all.sample.len() <= all.count as usize && !all.sample.is_empty());

    let tight = ScanLimits { max_entries: 2, ..roomy };
    let some = count_brought_back(&others, &after, &probe, &t.at("sub"), tight);
    assert!(some.capped, "{some:?}");
    assert!(some.count < 3, "a capped count is a lower bound: {some:?}");

    // Without rules there is nothing to bring back.
    let none = engine_of(&real_input(&[&t.root], &[]));
    assert_eq!(count_brought_back(&none, &none, &probe, &t.at("sub"), roomy).count, 0);
}

#[test]
fn brought_back_leaves_out_what_a_marker_file_hides() {
    // `cache` holds a marker, so restic backs up nothing else from it, `!sub` or not.
    let t = Tree::new("brought-back-marker", &["sub/c.txt", "sub/e.tmp", "sub/cache/.nobackup", "sub/cache/x.txt"]);
    let marked = |excludes: Vec<String>| TreeRulesInput {
        exclude_if_present: vec![".nobackup".to_string()],
        ..real_input(&[&t.root], &excludes)
    };
    let probe = FsProbe::default();
    let roomy = ScanLimits { budget: std::time::Duration::from_secs(30), max_entries: 1000 };

    let reinclude = format!("!{}", t.rule("sub"));
    let others = engine_of(&marked(lines_of(&["*.tmp"])));
    let after = engine_of(&marked(vec!["*.tmp".to_string(), reinclude.clone()]));
    assert!(others.judge_node(&t.at("sub/cache/x.txt"), &probe).is_some(), "the marker does hide x.txt");
    let found = count_brought_back(&others, &after, &probe, &t.at("sub"), roomy);
    assert_eq!((found.count, found.capped), (1, false), "only e.tmp is a pattern's doing: {found:?}");

    // Only markers hide anything: nothing for a `!` to bring back.
    let only_marker = engine_of(&marked(vec![]));
    let only_marker_after = engine_of(&marked(vec![reinclude]));
    assert_eq!(count_brought_back(&only_marker, &only_marker_after, &probe, &t.at("sub"), roomy).count, 0);

    // And through the edit itself.
    let i = marked(lines_of(&["*.tmp", "sub"]));
    let action = include(&t.at("sub"));
    let o = apply_edit(&i, &action, &probe, &no_read, &no_env).unwrap();
    assert_eq!(o.brought_back, Some(BroughtBack { count: 1, capped: false }));
    let i = marked(lines_of(&["sub"]));
    assert_eq!(apply_edit(&i, &action, &probe, &no_read, &no_env).unwrap().brought_back, None);
}

#[test]
fn brought_back_leaves_out_what_an_exclude_file_still_hides() {
    // The `!` on `sub` goes after the plan's own patterns but before every exclude file's lines,
    // and case-insensitive rules are a list of their own: what those hide inside `sub` stays out.
    let t = Tree::new("brought-back-file", &["sub/c.txt", "sub/e.tmp", "sub/d.log"]);
    let sub = t.at("sub");
    let run = |excludes: &[&str], file: &str, ignore_case: bool| {
        let i = TreeRulesInput {
            exclude_files: vec![crate::commands::cache::ExcludeFile { path: "/rules.txt".into(), ignore_case }],
            ..real_input(&[&t.root], &lines_of(excludes))
        };
        let read = |p: &str| if p == "/rules.txt" { Ok(file.to_string()) } else { Err(std::io::Error::other("missing")) };
        let o = apply_edit(&i, &include(&sub), &MockProbe::default(), &read, &no_env).unwrap();
        assert_eq!(o.added_rules.last().map(String::as_str), Some(format!("!{}", t.rule("sub")).as_str()), "{o:?}");
        o.brought_back
    };
    // Only an exclude file hides anything inside `sub`: nothing comes back with it.
    assert_eq!(run(&["sub"], "*.tmp\n", false), None);
    assert_eq!(run(&["sub"], "*.tmp\n", true), None);
    // The plan's own `*.log` is overridden by the `!` and counted; the file's `*.tmp` is not.
    assert_eq!(run(&["*.log", "sub"], "*.tmp\n", false), Some(BroughtBack { count: 1, capped: false }));
}

#[test]
fn brought_back_leaves_out_a_pattern_match_a_marker_file_also_hides() {
    // y.tmp matches the plan's `*.tmp`, which the `!` overrides, but the marker beside it still
    // hides it; e.tmp, which only `*.tmp` hides, does come back.
    let t = Tree::new("brought-back-both", &["sub/e.tmp", "sub/cache/.nobackup", "sub/cache/y.tmp"]);
    let i = TreeRulesInput {
        exclude_if_present: vec![".nobackup".to_string()],
        ..real_input(&[&t.root], &lines_of(&["*.tmp", "sub"]))
    };
    let o = apply_edit(&i, &include(&t.at("sub")), &FsProbe::default(), &no_read, &no_env).unwrap();
    assert_eq!(o.brought_back, Some(BroughtBack { count: 1, capped: false }));
}

// ── against the real restic ─────────────────────────────────────────────────

fn restic_available() -> bool {
    std::process::Command::new("restic")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `canonicalize` without the `\\?\` prefix Windows adds (restic, and our paths, don't use it).
fn plain_canonical(p: &std::path::Path) -> std::path::PathBuf {
    let c = p.canonicalize().unwrap();
    #[cfg(windows)]
    {
        if let Some(rest) = c.to_string_lossy().strip_prefix(r"\\?\") {
            return std::path::PathBuf::from(rest);
        }
    }
    c
}

/// The `/` spelling of a native path — how the engine and the comparisons below see it.
fn canon(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.to_string()
    }
}

/// A literal name or path as a pattern, escaped the way the platform's restic wants.
fn esc(s: &str) -> String {
    PathStyle::NATIVE.escape_glob(s)
}

fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("resty-plan-edit-{}-{label}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    plain_canonical(&dir)
}

/// Paths restic reports for a dry run over `sources` with the given extra args.
fn restic_included(repo: &std::path::Path, sources: &[String], args: &[String], cwd: &std::path::Path) -> HashSet<String> {
    let out = std::process::Command::new("restic")
        .args(["backup", "--repo"])
        .arg(repo)
        .args(["--dry-run", "-vv", "--json", "--no-scan"])
        .args(args)
        .args(sources)
        .current_dir(cwd)
        .env("RESTIC_PASSWORD", "pw")
        .output()
        .expect("run restic");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|j| j["message_type"] == "verbose_status")
        .filter_map(|j| j["item"].as_str().map(|i| crate::commands::test_support::restic_item_path(i, cfg!(windows)).trim_end_matches('/').to_string()))
        .collect()
}

const NAMES: &[&str] = &["a", "b", "c", "src", "deep", "node_modules", ".env", "x.log", "keep.txt", "we[ird]", "sp ace"];

fn grow(rng: &mut rand::rngs::StdRng, dir: &std::path::Path, depth: usize, all: &mut Vec<std::path::PathBuf>) {
    use rand::Rng;
    for _ in 0..rng.gen_range(1..=4) {
        let name = NAMES[rng.gen_range(0..NAMES.len())];
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        let is_file = (name.contains('.') && name != ".env") || depth >= 3 || rng.gen_range(0..4) == 0;
        if is_file {
            std::fs::write(&path, "x").unwrap();
            all.push(path);
        } else {
            std::fs::create_dir_all(&path).unwrap();
            all.push(path.clone());
            grow(rng, &path, depth + 1, all);
        }
    }
}

/// The number of cases and the seed of a differential run: `RESTY_DIFF_CASES` (default 80) and
/// `RESTY_DIFF_SEED` (default 3).
fn diff_cases_and_seed() -> (u64, u64) {
    let var = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    (var("RESTY_DIFF_CASES", 80), var("RESTY_DIFF_SEED", 3))
}

/// A fresh restic repository under `base`, for dry runs.
fn restic_repo(base: &std::path::Path) -> std::path::PathBuf {
    let repo = base.join("repo");
    let init = std::process::Command::new("restic").args(["init", "--repo"]).arg(&repo).env("RESTIC_PASSWORD", "pw").output().unwrap();
    assert!(init.status.success(), "restic init: {}", String::from_utf8_lossy(&init.stderr));
    repo
}

/// The patterns of a plan someone already wrote by hand over `nodes`: a few names, exact paths,
/// "everything inside", re-includes and an extension.
fn random_patterns(rng: &mut rand::rngs::StdRng, nodes: &[String]) -> Vec<String> {
    use rand::Rng;
    let mut excludes: Vec<String> = Vec::new();
    for _ in 0..rng.gen_range(1..5) {
        let n = &nodes[rng.gen_range(0..nodes.len())];
        let name = n.rsplit('/').next().unwrap();
        excludes.push(match rng.gen_range(0..5) {
            0 => esc(name),
            1 => esc(n),
            2 => format!("{}/*", esc(n)),
            3 => format!("!{}", esc(n)),
            _ => "*.log".to_string(),
        });
    }
    excludes
}

/// What restic includes when it is run the way the GUI runs it for `plan`: its patterns as
/// `--exclude` flags (comments dropped, exactly as at backup time) over its sources.
fn restic_plan(repo: &std::path::Path, plan: &TreeRulesInput, cwd: &std::path::Path) -> HashSet<String> {
    let args = crate::commands::snapshot::build_exclude_args(&plan.excludes, &[], false);
    restic_included(repo, &plan.sources, &args, cwd)
}

/// The engine's verdict on every node of `plan` matches restic's (`reported`).
fn assert_engine_agrees(plan: &TreeRulesInput, nodes: &[String], reported: &HashSet<String>, ctx: &str) {
    let read = |p: &str| std::fs::read_to_string(p);
    let (engine, _) = build_engine(plan, &read, &no_env).unwrap();
    for n in nodes {
        assert_eq!(
            engine.classify(n, &FsProbe::default()).included,
            reported.contains(n),
            "{ctx}: engine vs restic on {n}\nrules: {:?}",
            plan.excludes
        );
    }
}

/// The click the UI makes on `target`. On an included item: a left click excludes it, or a quarter
/// of the time the menu's "Exclude by pattern…" with its exact path or its name. On an excluded
/// one: a left click includes it, or a quarter of the time the menu's "include everything inside
/// it".
fn ui_click(rng: &mut rand::rngs::StdRng, target: &str, included: bool) -> EditAction {
    use rand::Rng;
    let from_the_menu = rng.gen_range(0..4) == 0;
    match (included, from_the_menu) {
        (false, false) => include(target),
        (false, true) => include_everything(target),
        (true, false) => exclude_item(target),
        (true, true) => {
            let name = target.rsplit('/').next().unwrap();
            exclude(target, &if rng.gen_range(0..2) == 0 { esc(target) } else { esc(name) })
        }
    }
}

/// Does `action` promise to change nothing but its target, what is inside it and the folders on
/// the way down to it? Every click does. A pattern does only when it is the target's exact path,
/// since a name pattern exists to match more.
fn held_to_its_target(action: &EditAction) -> bool {
    action.kind != "exclude" || action.pattern.as_deref() == Some(esc(&action.path).as_str())
}

/// The nodes restic judges differently in `before` and `after` that are not `target`, inside it,
/// or a folder on the way down to it.
fn collateral_nodes<'a>(nodes: &'a [String], target: &str, before: &HashSet<String>, after: &HashSet<String>) -> Vec<&'a str> {
    nodes
        .iter()
        .filter(|n| strip_source(n, target).is_none() && strip_source(target, n).is_none())
        .filter(|n| before.contains(*n) != after.contains(*n))
        .map(String::as_str)
        .collect()
}

/// The lines a click on `p` may delete: its exact rule, `!p`, and the `!A` / `A/*` scaffolding of
/// each folder `A` on the way down to it. (A tree comment naming `p` may go too, when it sits
/// directly above one of them.)
fn own_lines(p: &str) -> Vec<String> {
    let style = PathStyle::NATIVE;
    let rule = exact_rule(style, p);
    let mut own = vec![format!("!{rule}"), rule];
    let mut level = p.to_string();
    loop {
        let parent = style.parent(&level);
        if parent == level {
            return own;
        }
        level = parent;
        let a = exact_rule(style, &level);
        own.push(format!("{}/*", a.trim_end_matches('/')));
        own.push(format!("!{a}"));
    }
}

/// Asserts that a click on `p` turned `before` into `o.excludes` only by deleting `p`'s own lines
/// ([`own_lines`], and a tree comment naming `p` directly above a deleted one) and appending
/// `o.added_rules` under one tree comment naming `p`: every other line is still there, in order,
/// unchanged. Returns which lines of `before` were deleted.
fn assert_only_own_lines_deleted(before: &[String], o: &EditOutcome, p: &str, ctx: &str) -> Vec<bool> {
    let after = &o.excludes;
    let names_p = |l: &str| comment_names(l, Verb::Included, p) || comment_names(l, Verb::Excluded, p);
    let kept = if o.added_rules.is_empty() {
        &after[..]
    } else {
        let start = after.len().checked_sub(o.added_rules.len() + 1).unwrap_or_else(|| panic!("{ctx}: {after:?}"));
        assert_eq!(after[start + 1..], o.added_rules[..], "{ctx}: the added rules end the list");
        assert!(names_p(&after[start]), "{ctx}: no tree comment for {p} above the added rules: {after:?}");
        &after[..start]
    };
    let mut deleted = vec![false; before.len()];
    let mut j = 0;
    for (i, line) in before.iter().enumerate() {
        if kept.get(j) == Some(line) {
            j += 1;
        } else {
            deleted[i] = true;
        }
    }
    assert_eq!(j, kept.len(), "{ctx}: lines were rewritten or moved\nbefore {before:?}\nafter  {after:?}");
    let own = own_lines(p);
    for (i, line) in before.iter().enumerate().filter(|&(i, _)| deleted[i]) {
        let t = line.trim();
        let allowed = own.iter().any(|r| r == t) || (names_p(t) && deleted.get(i + 1) == Some(&true));
        assert!(allowed, "{ctx}: deleted {line:?}, which isn't {p}'s own\nbefore {before:?}\nafter  {after:?}");
    }
    deleted
}

/// A tree edit was refused. Refusing because it would change something else is never right for a
/// click the UI offers (the engine must find an edit that doesn't), so that is a failure; any other
/// refusal says the click is impossible (a marker, a rule the plan can't override, …).
fn refused_as_impossible(err: &str, ctx: &str, plan: &TreeRulesInput) {
    assert!(!err.contains("would also change"), "{ctx}: refused as collateral, which is an engine bug: {err}\nrules: {:?}", plan.excludes);
}

/// Applies random clicks to random plans on random trees and checks, with the real restic, that
/// (a) restic agrees the clicked item ended up as asked, (b) the engine agrees with restic about
/// every node and (c) nothing else changed: every node that is not the item, inside it or on the
/// way down to it has the same restic verdict as before (not checked for a name pattern, which is
/// meant to match more). The clicks are the UI's: see [`ui_click`]. Needs `restic` on PATH:
/// `cargo test restic_confirms -- --ignored --nocapture`.
#[test]
#[ignore]
fn restic_confirms_every_edit_the_engine_offers() {
    use rand::{Rng, SeedableRng};
    if !restic_available() {
        eprintln!("restic not found on PATH; skipping");
        return;
    }
    let base = scratch("edits");
    let repo = restic_repo(&base);
    let (cases, seed) = diff_cases_and_seed();
    let judge = crate::commands::test_support::restic_judges_sources();
    let probe = FsProbe::default();
    let read = |p: &str| std::fs::read_to_string(p);
    let (mut applied, mut refused, mut nodes_checked) = (0, 0, 0usize);
    let mut by_kind: HashMap<String, usize> = HashMap::new();
    for case in 0..cases {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed * 1000 + case);
        let root = base.join(format!("t{case}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = plain_canonical(&root);
        let mut paths = Vec::new();
        grow(&mut rng, &root, 0, &mut paths);
        if paths.is_empty() {
            continue;
        }
        let nodes: Vec<String> = paths.iter().map(|p| canon(p.to_str().unwrap())).collect();
        let rs = canon(root.to_str().unwrap());

        // An existing plan: a few random patterns (names, exact paths, contents-of, a re-include).
        let excludes = random_patterns(&mut rng, &nodes);
        let plan = TreeRulesInput { sources: vec![rs.clone()], excludes: excludes.clone(), judge_sources: judge, ..Default::default() };
        let (before, _) = match build_engine(&plan, &read, &no_env) {
            Ok(x) => x,
            Err(_) => continue,
        };

        // Mostly click things that are excluded (the interesting direction: re-including, often
        // from deep inside an excluded folder); sometimes click anything.
        let excluded_nodes: Vec<&String> = nodes.iter().filter(|p| !before.classify(p, &probe).included).collect();
        let target = if !excluded_nodes.is_empty() && rng.gen_range(0..10) < 7 {
            excluded_nodes[rng.gen_range(0..excluded_nodes.len())].clone()
        } else {
            nodes[rng.gen_range(0..nodes.len())].clone()
        };
        let target_included = before.classify(&target, &probe).included;
        let action = ui_click(&mut rng, &target, target_included);
        let ctx = format!("case {case} (seed {seed}): {} {target} {:?}", action.kind, action.pattern);

        let out = match apply_edit(&plan, &action, &probe, &read, &no_env) {
            Ok(o) => o,
            Err(e) => {
                refused_as_impossible(&e, &ctx, &plan);
                refused += 1;
                continue;
            }
        };
        applied += 1;
        *by_kind.entry(action.kind.clone()).or_default() += 1;

        // What the GUI would run, before and after: the patterns as --exclude flags over the sources.
        let reported_before = restic_plan(&repo, &plan, &base);
        let after_plan = TreeRulesInput { sources: out.sources.clone(), excludes: out.excludes.clone(), ..plan.clone() };
        let reported = restic_plan(&repo, &after_plan, &base);
        let want = !target_included; // include flips it in, exclude flips it out
        assert_eq!(
            reported.contains(&target),
            want,
            "{ctx}\nbefore: {excludes:?}\nafter:  {:?}\nsources {:?}",
            out.excludes,
            out.sources
        );

        // The engine still agrees with restic about every node…
        assert_engine_agrees(&after_plan, &nodes, &reported, &ctx);
        nodes_checked += nodes.len();

        // …and nothing but the item, what is inside it and the way down to it changed.
        if held_to_its_target(&action) {
            let hit = collateral_nodes(&nodes, &target, &reported_before, &reported);
            assert!(hit.is_empty(), "{ctx}: also changed {hit:?}\nbefore: {excludes:?}\nafter:  {:?}", out.excludes);
        }
    }
    eprintln!("restic confirmed {applied} edits ({by_kind:?}; {refused} refused as impossible), {nodes_checked} nodes agree");
    assert!(applied > 0);
    let _ = std::fs::remove_dir_all(&base);
}

/// One click of a sequence, kept to check the next click on the same item against it.
struct Click {
    target: String,
    action: EditAction,
    /// The plan's patterns before the click, and which of them it deleted.
    before: Vec<String>,
    deleted: Vec<bool>,
    outcome: EditOutcome,
    /// What restic included before the click.
    reported_before: HashSet<String>,
}

/// How a click and the inverse click on the same item came back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RoundTrip {
    /// The exact rule list the first click started from, comments included.
    Identical,
    /// The same patterns (the first click deleted lines from the end, the second wrote them back,
    /// under its own comment).
    SameRules,
    /// The same included set (the first click deleted lines the second can't put back in place).
    SameSet,
    /// Neither applies (see [`check_round_trip`]); the item is in the state the second click asked
    /// for, which every click is checked for.
    StateOnly,
}

/// Checks a click (`first`) followed by the inverse click on the same item (`second`, which left
/// restic including `reported`):
/// - the first click only appended rules: the second deletes exactly them, so the rule list is the
///   one the first started from, comments included;
/// - it deleted lines from the end of the list, no two of them alike, and the second only
///   appended: the same patterns (two copies of one line would come back as one);
/// - an include that deleted lines anywhere else (or deleted a dead exact line and added `!P`): the
///   same included set, when no `!` line comes after a deleted one (a hand-written line in the middle
///   can't be put back in place, and moving an exclude past a `!` would change what that `!` does);
/// - an excludeItem that deleted lines the include can't write back (a re-include that is not the
///   last line, or `!P` where the include deletes P's exact line instead): only the state.
fn check_round_trip(first: &Click, second: &EditOutcome, reported: &HashSet<String>, ctx: &str) -> RoundTrip {
    let (l0, o1) = (&first.before, &first.outcome);
    let l2 = &second.excludes;
    let what = || format!("{ctx}: {} then the inverse\nstart:  {l0:?}\nfirst:  {:?}\nsecond: {l2:?}", first.action.kind, o1.excludes);
    if !held_to_its_target(&first.action) {
        return RoundTrip::StateOnly; // a name pattern is not what a click on the item undoes
    }
    if o1.removed_rules.is_empty() {
        assert_eq!(l2, l0, "{}", what());
        return RoundTrip::Identical;
    }
    let mut once = o1.removed_rules.clone();
    once.sort();
    once.dedup();
    let from_the_end = o1.added_rules.is_empty() && l0.starts_with(&o1.excludes);
    if from_the_end && second.removed_rules.is_empty() && once.len() == o1.removed_rules.len() {
        assert_eq!(patterns(l2), patterns(l0), "{}", what());
        return RoundTrip::SameRules;
    }
    if first.action.kind == "include" {
        let first_deleted = first.deleted.iter().position(|&d| d).unwrap();
        if !l0[first_deleted..].iter().any(|l| l.trim_start().starts_with('!')) {
            let (start, end) = (&first.reported_before, reported);
            assert_eq!(start, end, "{}\nincluded only at the start: {:?}\nonly at the end: {:?}", what(), start.difference(end).collect::<Vec<_>>(), end.difference(start).collect::<Vec<_>>());
            return RoundTrip::SameSet;
        }
    }
    RoundTrip::StateOnly
}

/// Runs of 2–6 clicks per case on random plans over random trees, each click starting from the plan
/// the one before it left, and checked with the real restic after every click: the item ended up
/// as asked, the engine agrees with restic about every node, nothing else changed (as in
/// [`restic_confirms_every_edit_the_engine_offers`]), and the click deleted only the item's own lines
/// (every other line is still there, in order). Half the time the next click is the inverse click
/// on the same item (a left click on it again), and the pair must come back to where it started
/// ([`check_round_trip`]). Some plans start with a comment of the user's. Needs `restic` on PATH:
/// `cargo test restic_confirms -- --ignored --nocapture`.
#[test]
#[ignore]
fn restic_confirms_click_sequences() {
    use rand::{Rng, SeedableRng};
    if !restic_available() {
        eprintln!("restic not found on PATH; skipping");
        return;
    }
    let base = scratch("sequences");
    let repo = restic_repo(&base);
    let (cases, seed) = diff_cases_and_seed();
    let judge = crate::commands::test_support::restic_judges_sources();
    let probe = FsProbe::default();
    let read = |p: &str| std::fs::read_to_string(p);
    let (mut applied, mut refused, mut nodes_checked) = (0, 0, 0usize);
    let mut by_kind: HashMap<String, usize> = HashMap::new();
    let mut round_trips: HashMap<RoundTrip, usize> = HashMap::new();
    for case in 0..cases {
        let mut rng = rand::rngs::StdRng::seed_from_u64(500_000 + seed * 1000 + case);
        let root = base.join(format!("t{case}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = plain_canonical(&root);
        let mut paths = Vec::new();
        grow(&mut rng, &root, 0, &mut paths);
        if paths.is_empty() {
            continue;
        }
        let nodes: Vec<String> = paths.iter().map(|p| canon(p.to_str().unwrap())).collect();

        let mut excludes = random_patterns(&mut rng, &nodes);
        if rng.gen_range(0..3) == 0 {
            let at = rng.gen_range(0..=excludes.len());
            excludes.insert(at, "# my own note".to_string());
        }
        let mut plan = TreeRulesInput { sources: vec![canon(root.to_str().unwrap())], excludes, judge_sources: judge, ..Default::default() };
        if build_engine(&plan, &read, &no_env).is_err() {
            continue;
        }
        let mut reported = restic_plan(&repo, &plan, &base);
        assert_engine_agrees(&plan, &nodes, &reported, &format!("case {case} (seed {seed}) at the start"));

        let mut prev: Option<Click> = None;
        for step in 0..rng.gen_range(2..=6) {
            let (engine, _) = build_engine(&plan, &read, &no_env).unwrap();
            let again = prev.as_ref().map(|c| c.target.clone()).filter(|_| rng.gen_range(0..2) == 0);
            let inverse = again.is_some();
            let target = again.unwrap_or_else(|| nodes[rng.gen_range(0..nodes.len())].clone());
            let included = engine.classify(&target, &probe).included;
            let action = match (inverse, included) {
                (true, true) => exclude_item(&target),
                (true, false) => include(&target),
                (false, _) => ui_click(&mut rng, &target, included),
            };
            let ctx = format!("case {case} step {step} (seed {seed}): {} {target} {:?}", action.kind, action.pattern);
            let out = match apply_edit(&plan, &action, &probe, &read, &no_env) {
                Ok(o) => o,
                Err(e) => {
                    // The click that undoes a click on the same item must always be possible (a
                    // name pattern aside: a click on the item is not what undoes it).
                    let undoes = prev.as_ref().filter(|c| inverse && held_to_its_target(&c.action));
                    if let Some(first) = undoes {
                        panic!("{ctx}: the inverse of {} was refused: {e}\nrules: {:?}", first.action.kind, plan.excludes);
                    }
                    refused_as_impossible(&e, &ctx, &plan);
                    refused += 1;
                    prev = None;
                    continue;
                }
            };
            applied += 1;
            *by_kind.entry(action.kind.clone()).or_default() += 1;

            let next = TreeRulesInput { sources: out.sources.clone(), excludes: out.excludes.clone(), ..plan.clone() };
            let after = restic_plan(&repo, &next, &base);
            let ctx = format!("{ctx}\nbefore: {:?}\nafter:  {:?}", plan.excludes, out.excludes);
            assert_eq!(after.contains(&target), !included, "{ctx}: the item isn't as asked");
            assert_engine_agrees(&next, &nodes, &after, &ctx);
            nodes_checked += nodes.len();
            if held_to_its_target(&action) {
                let hit = collateral_nodes(&nodes, &target, &reported, &after);
                assert!(hit.is_empty(), "{ctx}: also changed {hit:?}");
            }
            let deleted = assert_only_own_lines_deleted(&plan.excludes, &out, &target, &ctx);
            if let Some(first) = prev.as_ref().filter(|_| inverse) {
                *round_trips.entry(check_round_trip(first, &out, &after, &ctx)).or_default() += 1;
            }
            prev = Some(Click { target, action, before: plan.excludes.clone(), deleted, outcome: out, reported_before: reported });
            plan = next;
            reported = after;
        }
    }
    eprintln!(
        "restic confirmed {applied} clicks in {cases} sequences ({by_kind:?}; {refused} refused as impossible), \
         {} round trips {round_trips:?}, {nodes_checked} nodes agree",
        round_trips.values().sum::<usize>()
    );
    assert!(applied > 0 && !round_trips.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// The exported files must behave exactly like the flags the GUI passes: the same included set
/// from `restic backup --exclude-file …` as from `--exclude …` flags (and the case-insensitive
/// file as `--iexclude` flags).
#[test]
#[ignore]
fn restic_confirms_exported_files_match_the_gui_flags() {
    use rand::{Rng, SeedableRng};
    if !restic_available() {
        eprintln!("restic not found on PATH; skipping");
        return;
    }
    let base = scratch("export");
    let repo = base.join("repo");
    assert!(std::process::Command::new("restic").args(["init", "--repo"]).arg(&repo).env("RESTIC_PASSWORD", "pw").output().unwrap().status.success());
    let cases: u64 = std::env::var("RESTY_DIFF_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let mut compared = 0usize;
    for case in 0..cases {
        let mut rng = rand::rngs::StdRng::seed_from_u64(77_000 + case);
        let root = base.join(format!("t{case}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = plain_canonical(&root);
        let mut nodes = Vec::new();
        grow(&mut rng, &root, 0, &mut nodes);
        if nodes.is_empty() {
            continue;
        }
        let rs = canon(root.to_str().unwrap());
        let mut excludes = Vec::new();
        for _ in 0..rng.gen_range(1..5) {
            let n = canon(nodes[rng.gen_range(0..nodes.len())].to_str().unwrap());
            let name = n.rsplit('/').next().unwrap().to_string();
            excludes.push(match rng.gen_range(0..6) {
                0 => esc(&name),
                1 => esc(&n),
                2 => format!("{}/*", esc(&n)),
                3 => format!("!{}", esc(&n)),
                4 => format!("**/{}", esc(&name)),
                _ => "*.log".to_string(),
            });
        }
        let plan = TreeRulesInput { sources: vec![rs.clone()], excludes: excludes.clone(), ..Default::default() };
        let read = |p: &str| std::fs::read_to_string(p);
        let built = build_rules(&plan, &read, &no_env);
        let export = export_rules(&built.rules);

        let dir = base.join(format!("export{case}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(&export.exclude_file_name), &export.exclude_file).unwrap();

        let flags: Vec<String> = excludes.iter().flat_map(|p| ["--exclude".to_string(), p.clone()]).collect();
        let files = vec!["--exclude-file".to_string(), export.exclude_file_name.clone()];
        let via_flags = restic_included(&repo, &built.rules.sources, &flags, &dir);
        let via_files = restic_included(&repo, &built.rules.sources, &files, &dir);
        assert_eq!(via_flags, via_files, "case {case}: {excludes:?}\nexported:\n{}", export.exclude_file);
        compared += via_flags.len();
    }
    eprintln!("exported files and GUI flags agree on {compared} reported nodes over {cases} cases");
    let _ = std::fs::remove_dir_all(&base);
}
