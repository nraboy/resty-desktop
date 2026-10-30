use std::collections::{HashMap, HashSet};

use super::lines::{comment_names, Verb};
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

// The `excludeItem` edit (a left click on an included item) isn't handled by the engine yet.
#[allow(dead_code)]
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
/// directly above the first of them; an edit that only deletes lines leaves no tree comment for
/// `p` behind.
fn assert_tree_comment(o: &EditOutcome, p: &str) {
    assert!(o.added_rules.iter().chain(&o.removed_rules).all(|r| !r.trim_start().starts_with('#')), "{o:?}");
    let names_p = |l: &String| comment_names(l, Verb::Included, p) || comment_names(l, Verb::Excluded, p);
    if o.added_rules.is_empty() {
        assert!(!o.excludes.iter().any(names_p), "a tree comment for {p} is left: {:?}", o.excludes);
        return;
    }
    let start = o.excludes.len() - o.added_rules.len();
    assert_eq!(o.excludes[start..], o.added_rules[..], "the added rules end the list");
    assert!(start > 0 && comment_names(&o.excludes[start - 1], Verb::Included, p), "no tree comment for {p} above {:?}", o.excludes);
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

/// Is `path` included after applying `outcome` to `i`?
fn included_after(i: &TreeRulesInput, o: &EditOutcome, path: &str) -> bool {
    let mut j = i.clone();
    j.sources = o.sources.clone();
    j.excludes = o.excludes.clone();
    let (e, _) = build_engine(&j, &no_read, &no_env).unwrap();
    e.classify(path, &MockProbe::default()).included
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
    assert_eq!(o.excludes, ["/s/a"]);
    assert!(!included_after(&i, &o, "/s/a/x"));
    assert!(included_after(&i, &o, "/s/b"));
}

#[test]
fn excluding_by_name_hits_every_folder_of_that_name() {
    let i = input(&["/s"], &[]);
    let o = edit(&i, &exclude("/s/a/node_modules", "node_modules")).unwrap();
    assert!(!included_after(&i, &o, "/s/a/node_modules"));
    assert!(!included_after(&i, &o, "/s/c/deep/node_modules/y.js"));
    assert!(included_after(&i, &o, "/s/c/deep/code.py"));
}

#[test]
fn the_new_rule_goes_last_so_it_beats_earlier_reincludes() {
    let i = input(&["/s"], &["/s/*", "!/s/a"]);
    let o = edit(&i, &exclude("/s/a/keep.txt", "/s/a/keep.txt")).unwrap();
    assert_eq!(o.excludes, ["/s/*", "!/s/a", "/s/a/keep.txt"]);
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

/// Applies random include/exclude clicks to random plans on random trees and checks, with the
/// real restic, that (a) restic agrees the clicked item ended up as asked and (b) the engine
/// agrees with restic about every node. Needs `restic` on PATH:
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
    let repo = base.join("repo");
    assert!(std::process::Command::new("restic").args(["init", "--repo"]).arg(&repo).env("RESTIC_PASSWORD", "pw").output().unwrap().status.success());

    let cases: u64 = std::env::var("RESTY_DIFF_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(80);
    let seed: u64 = std::env::var("RESTY_DIFF_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let (mut applied, mut refused, mut nodes_checked) = (0, 0, 0usize);
    let mut by_kind: HashMap<&str, usize> = HashMap::new();
    for case in 0..cases {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed * 1000 + case);
        let root = base.join(format!("t{case}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = plain_canonical(&root);
        let mut nodes = Vec::new();
        grow(&mut rng, &root, 0, &mut nodes);
        if nodes.is_empty() {
            continue;
        }
        let rs = canon(root.to_str().unwrap());

        // An existing plan: a few random patterns (names, exact paths, contents-of, a re-include).
        let mut excludes: Vec<String> = Vec::new();
        for _ in 0..rng.gen_range(1..5) {
            let n = canon(nodes[rng.gen_range(0..nodes.len())].to_str().unwrap());
            let name = n.rsplit('/').next().unwrap().to_string();
            excludes.push(match rng.gen_range(0..5) {
                0 => esc(&name),
                1 => esc(&n),
                2 => format!("{}/*", esc(&n)),
                3 => format!("!{}", esc(&n)),
                _ => "*.log".to_string(),
            });
        }
        let judge = crate::commands::test_support::restic_judges_sources();
        let plan = TreeRulesInput { sources: vec![rs.clone()], excludes: excludes.clone(), judge_sources: judge, ..Default::default() };
        let probe = FsProbe::default();
        let read = |p: &str| std::fs::read_to_string(p);
        let (before, _) = match build_engine(&plan, &read, &no_env) {
            Ok(x) => x,
            Err(_) => continue,
        };

        // Mostly click things that are excluded (the interesting direction: re-including, often
        // from deep inside an excluded folder); sometimes click anything.
        let excluded_nodes: Vec<&std::path::PathBuf> =
            nodes.iter().filter(|p| !before.classify(p.to_str().unwrap(), &probe).included).collect();
        let target = if !excluded_nodes.is_empty() && rng.gen_range(0..10) < 7 {
            canon(excluded_nodes[rng.gen_range(0..excluded_nodes.len())].to_str().unwrap())
        } else {
            canon(nodes[rng.gen_range(0..nodes.len())].to_str().unwrap())
        };
        let target_included = before.classify(&target, &probe).included;
        let name = target.rsplit('/').next().unwrap().to_string();
        let action = if target_included {
            let pattern = if rng.gen_range(0..2) == 0 { esc(&target) } else { esc(&name) };
            exclude(&target, &pattern)
        } else {
            include(&target)
        };
        let kind = if target_included { "exclude" } else { "include" };

        let out = match apply_edit(&plan, &action, &probe, &read, &no_env) {
            Ok(o) => o,
            Err(_) => {
                refused += 1;
                continue;
            }
        };
        applied += 1;
        *by_kind.entry(kind).or_default() += 1;

        // What the GUI would run: the new patterns as --exclude flags over the new sources.
        let args: Vec<String> = out.excludes.iter().flat_map(|p| ["--exclude".to_string(), p.clone()]).collect();
        let reported = restic_included(&repo, &out.sources, &args, &base);
        let want = !target_included; // include flips it in, exclude flips it out
        assert_eq!(
            reported.contains(&target),
            want,
            "case {case} (seed {seed}): {kind} {target}\nbefore: {excludes:?}\nafter:  {:?}\nsources {:?}",
            out.excludes,
            out.sources
        );

        // And the engine still agrees with restic about every node.
        let after_plan = TreeRulesInput { sources: out.sources.clone(), excludes: out.excludes.clone(), judge_sources: judge, ..Default::default() };
        let (after, _) = build_engine(&after_plan, &read, &no_env).unwrap();
        for node in nodes.iter().filter(|p| p.exists()) {
            let p = canon(node.to_str().unwrap());
            let p = p.as_str();
            assert_eq!(
                after.classify(p, &probe).included,
                reported.contains(p),
                "case {case} (seed {seed}): engine vs restic on {p}\nrules: {:?}",
                out.excludes
            );
            nodes_checked += 1;
        }
    }
    eprintln!("restic confirmed {applied} edits ({by_kind:?}; {refused} refused as impossible), {nodes_checked} nodes agree");
    assert!(applied > 0);
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
