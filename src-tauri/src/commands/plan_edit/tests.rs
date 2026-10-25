use std::collections::{HashMap, HashSet};

use super::*;
use crate::commands::plan_rules::{FsProbe, Marker};

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
        ..Default::default()
    }
}

fn exclude(path: &str, pattern: &str) -> EditAction {
    EditAction { kind: "exclude".into(), path: path.into(), pattern: Some(pattern.into()), scope: None }
}

fn include(path: &str, scope: Option<&str>) -> EditAction {
    EditAction { kind: "include".into(), path: path.into(), pattern: None, scope: scope.map(String::from) }
}

fn edit(i: &TreeRulesInput, a: &EditAction) -> Result<EditOutcome, String> {
    apply_edit(i, a, &MockProbe::default(), &no_read, &no_env)
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
    assert_eq!(carve_out_chain("/s/a", "/s/a"), ["!/s/a"]);
    assert_eq!(
        carve_out_chain("/s/a", "/s/a/b/c"),
        ["!/s/a", "/s/a/*", "!/s/a/b", "/s/a/b/*", "!/s/a/b/c"]
    );
    assert_eq!(carve_out_chain("/", "/x"), ["!/", "/*", "!/x"]);
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
    let o = edit(&i, &include("/home/u/code", None)).unwrap();
    assert_eq!(o.added_sources, ["/home/u/code"]);
    assert_eq!(o.removed_sources, ["/home/u/code/app", "/home/u/code/lib"]);
    assert_eq!(o.sources, ["/home/u/code"]);
    assert!(included_after(&i, &o, "/home/u/code/app/x"));

    let o = edit(&input(&["/s"], &[]), &include("/elsewhere/notes.txt", None)).unwrap();
    assert_eq!(o.sources, ["/s", "/elsewhere/notes.txt"]);
}

#[test]
fn including_a_folder_excluded_by_name_re_includes_it_whole() {
    let i = input(&["/s"], &["node_modules"]);
    let o = edit(&i, &include("/s/a/node_modules", None)).unwrap();
    assert_eq!(o.added_rules, ["!/s/a/node_modules"]);
    assert!(included_after(&i, &o, "/s/a/node_modules"));
    assert!(included_after(&i, &o, "/s/a/node_modules/x.js"));
    assert!(!included_after(&i, &o, "/s/c/deep/node_modules"));
}

#[test]
fn including_one_item_inside_an_excluded_folder_carves_it_out() {
    let i = input(&["/s"], &["node_modules"]);
    let o = edit(&i, &include("/s/a/node_modules/lib/index.js", None)).unwrap();
    assert_eq!(
        o.added_rules,
        [
            "!/s/a/node_modules",
            "/s/a/node_modules/*",
            "!/s/a/node_modules/lib",
            "/s/a/node_modules/lib/*",
            "!/s/a/node_modules/lib/index.js"
        ]
    );
    assert!(included_after(&i, &o, "/s/a/node_modules/lib/index.js"));
    // Everything else in there is still out, and so is every other node_modules.
    assert!(!included_after(&i, &o, "/s/a/node_modules/lib/other.js"));
    assert!(!included_after(&i, &o, "/s/a/node_modules/other"));
    assert!(!included_after(&i, &o, "/s/c/node_modules/lib/index.js"));
}

#[test]
fn the_whole_folder_choice_brings_back_the_excluded_folder() {
    let i = input(&["/s"], &["node_modules"]);
    let o = edit(&i, &include("/s/a/node_modules/lib/index.js", Some("folder"))).unwrap();
    assert_eq!(o.added_rules, ["!/s/a/node_modules"]);
    assert!(included_after(&i, &o, "/s/a/node_modules/anything"));
}

#[test]
fn including_a_source_whose_own_path_matches_a_name_pattern_re_includes_its_contents() {
    // The source root is exempt from the patterns, but `node_modules` matches every path inside
    // it, so nothing in the folder would be backed up until it is re-included.
    let i = input(&["/s/a/node_modules"], &["node_modules"]);
    let o = edit(&i, &include("/s/a/node_modules", None)).unwrap();
    assert_eq!(o.added_rules, ["!/s/a/node_modules"]);
    assert!(included_after(&i, &o, "/s/a/node_modules/x.js"));
}

#[test]
fn already_included_things_are_refused() {
    let i = input(&["/s"], &["node_modules"]);
    assert!(edit(&i, &include("/s/a", None)).unwrap_err().contains("already included"));
    assert!(edit(&i, &include("/s", None)).unwrap_err().contains("already included"));
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
    let err = apply_edit(&i, &include("/s/a/keep.txt", None), &MockProbe::default(), &read, &no_env).unwrap_err();
    assert!(err.contains("line 2 of /rules.txt"), "{err}");
    assert!(err.contains("applied after the plan's own patterns"), "{err}");
}

#[test]
fn a_case_insensitive_file_rule_cannot_be_cancelled() {
    let mut i = input(&["/s"], &[]);
    i.exclude_files = vec![crate::commands::cache::ExcludeFile { path: "/ci.txt".into(), ignore_case: true }];
    let read = |_: &str| Ok("Thumbs.db\n".to_string());
    let err = apply_edit(&i, &include("/s/x/thumbs.db", None), &MockProbe::default(), &read, &no_env).unwrap_err();
    assert!(err.contains("case-insensitive") || err.contains("line 1"), "{err}");
}

#[test]
fn a_marker_file_exclusion_cannot_be_overridden_by_patterns() {
    let mut i = input(&["/s"], &[]);
    i.exclude_if_present = vec![".nobackup".into()];
    let mut probe = MockProbe::default();
    probe.markers.insert(("/s/b".into(), ".nobackup".into()));
    let err = apply_edit(&i, &include("/s/b/data.bin", None), &probe, &no_read, &no_env).unwrap_err();
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

fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("resty-plan-edit-{}-{label}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
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
        .filter_map(|j| j["item"].as_str().map(|i| i.trim_end_matches('/').to_string()))
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
        let root = root.canonicalize().unwrap();
        let mut nodes = Vec::new();
        grow(&mut rng, &root, 0, &mut nodes);
        if nodes.is_empty() {
            continue;
        }
        let rs = root.to_str().unwrap().to_string();

        // An existing plan: a few random patterns (names, exact paths, contents-of, a re-include).
        let mut excludes: Vec<String> = Vec::new();
        for _ in 0..rng.gen_range(1..5) {
            let n = nodes[rng.gen_range(0..nodes.len())].to_str().unwrap().to_string();
            let name = n.rsplit('/').next().unwrap().to_string();
            excludes.push(match rng.gen_range(0..5) {
                0 => escape_glob(&name),
                1 => escape_glob(&n),
                2 => format!("{}/*", escape_glob(&n)),
                3 => format!("!{}", escape_glob(&n)),
                _ => "*.log".to_string(),
            });
        }
        let plan = TreeRulesInput { sources: vec![rs.clone()], excludes: excludes.clone(), ..Default::default() };
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
            excluded_nodes[rng.gen_range(0..excluded_nodes.len())].to_str().unwrap().to_string()
        } else {
            nodes[rng.gen_range(0..nodes.len())].to_str().unwrap().to_string()
        };
        let target_included = before.classify(&target, &probe).included;
        let name = target.rsplit('/').next().unwrap().to_string();
        let action = if target_included {
            let pattern = if rng.gen_range(0..2) == 0 { escape_glob(&target) } else { escape_glob(&name) };
            exclude(&target, &pattern)
        } else {
            include(&target, if rng.gen_range(0..3) == 0 { Some("folder") } else { None })
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
        let after_plan = TreeRulesInput { sources: out.sources.clone(), excludes: out.excludes.clone(), ..Default::default() };
        let (after, _) = build_engine(&after_plan, &read, &no_env).unwrap();
        for node in nodes.iter().filter(|p| p.exists()) {
            let p = node.to_str().unwrap();
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
        let root = root.canonicalize().unwrap();
        let mut nodes = Vec::new();
        grow(&mut rng, &root, 0, &mut nodes);
        if nodes.is_empty() {
            continue;
        }
        let rs = root.to_str().unwrap().to_string();
        let mut excludes = Vec::new();
        for _ in 0..rng.gen_range(1..5) {
            let n = nodes[rng.gen_range(0..nodes.len())].to_str().unwrap().to_string();
            let name = n.rsplit('/').next().unwrap().to_string();
            excludes.push(match rng.gen_range(0..6) {
                0 => escape_glob(&name),
                1 => escape_glob(&n),
                2 => format!("{}/*", escape_glob(&n)),
                3 => format!("!{}", escape_glob(&n)),
                4 => format!("**/{}", escape_glob(&name)),
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
