use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use super::*;

fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::AtomicUsize;
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "resty-plan-tree-{}-{label}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x").unwrap();
}

fn input(sources: &[&str], excludes: &[&str]) -> TreeRulesInput {
    TreeRulesInput {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        excludes: excludes.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn no_files(_: &str) -> std::io::Result<String> {
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
}

fn no_env(_: &str) -> String {
    String::new()
}

// ── env expansion (Go's os.Expand with restic's mapping) ────────────────────

#[test]
fn expand_env_matches_gos_os_expand() {
    let env: HashMap<&str, &str> = HashMap::from([("HOME", "/home/u"), ("X", "1")]);
    let get = |k: &str| env.get(k).copied().unwrap_or("").to_string();
    for (input, want) in [
        ("$HOME/.cache", "/home/u/.cache"),
        ("${HOME}/x", "/home/u/x"),
        ("a$$b", "a$b"),
        ("cost$$", "cost$"),
        ("trailing$", "trailing$"),
        ("$UNSET/x", "/x"),
        ("$X$X", "11"),
        ("${}x", "x"),
        ("${unclosed", "unclosed"),
        ("no dollars", "no dollars"),
        ("$ space", "$ space"),
        ("$1abc", "abc"),
    ] {
        assert_eq!(expand_env(input, &get), want, "Expand({input:?})");
    }
}

// ── building rules ──────────────────────────────────────────────────────────

#[test]
fn flags_are_trimmed_and_comments_and_blanks_dropped_like_the_backup_does() {
    let built = build_rules(&input(&["/s"], &["  node_modules ", "", "# note", "*.log"]), &no_files, &no_env);
    let pats: Vec<&str> = built.rules.rules.iter().map(|r| r.pattern.as_str()).collect();
    assert_eq!(pats, ["node_modules", "*.log"]);
    assert!(built.problems.is_empty());
}

#[test]
fn exclude_files_extend_the_flags_in_order_with_line_numbers_and_env_expansion() {
    let mut i = input(&["/s"], &["flag-pattern"]);
    i.exclude_files = vec![
        ExcludeFile { path: "/cs.txt".into(), ignore_case: false },
        ExcludeFile { path: "/ci.txt".into(), ignore_case: true },
    ];
    let files: HashMap<&str, &str> = HashMap::from([
        ("/cs.txt", "# header\n\n  $BASE/skip  \n!keep\n"),
        ("/ci.txt", "Thumbs.DB\n"),
    ]);
    let read = |p: &str| files.get(p).map(|s| s.to_string()).ok_or_else(|| std::io::Error::other("missing"));
    let getenv = |k: &str| if k == "BASE" { "/data".to_string() } else { String::new() };
    let built = build_rules(&i, &read, &getenv);
    let got: Vec<(u32, &str, bool, Option<u32>)> = built
        .rules
        .rules
        .iter()
        .zip(&built.origins)
        .map(|(r, o)| (r.id, r.pattern.as_str(), r.ignore_case, o.line))
        .collect();
    assert_eq!(
        got,
        [
            (1, "flag-pattern", false, None),
            (2, "/data/skip", false, Some(3)),
            (3, "!keep", false, Some(4)),
            (4, "Thumbs.DB", true, Some(1)),
        ]
    );
    assert_eq!(built.origins[1].file.as_deref(), Some("/cs.txt"));
}

#[test]
fn unreadable_files_and_invalid_patterns_are_reported_not_fatal() {
    let mut i = input(&["/s"], &["[bad", "ok"]);
    i.exclude_files = vec![ExcludeFile { path: "/missing.txt".into(), ignore_case: false }];
    i.exclude_if_present = vec![":nameless".into(), ".nobackup".into()];
    let built = build_rules(&i, &no_files, &no_env);
    assert_eq!(built.rules.rules.len(), 1);
    assert_eq!(built.rules.markers, [".nobackup"]);
    let whats: Vec<&str> = built.problems.iter().map(|p| p.what.as_str()).collect();
    assert_eq!(whats, ["[bad", "/missing.txt", ":nameless"]);
    // Still a usable engine.
    assert!(Engine::new(&built.rules).is_ok());
}

// ── listing ─────────────────────────────────────────────────────────────────

#[test]
fn list_dir_sorts_folders_first_case_insensitively_and_caps() {
    let dir = scratch("list");
    std::fs::create_dir_all(dir.join("zeta")).unwrap();
    std::fs::create_dir_all(dir.join("Alpha")).unwrap();
    touch(&dir.join("beta.txt"));
    touch(&dir.join("Apple.txt"));
    std::os::unix::fs::symlink(dir.join("zeta"), dir.join("link")).unwrap();

    let l = list_dir(dir.to_str().unwrap(), 100);
    let names: Vec<(&str, &str)> = l.entries.iter().map(|e| (e.name.as_str(), e.kind)).collect();
    assert_eq!(
        names,
        [("Alpha", "dir"), ("zeta", "dir"), ("Apple.txt", "file"), ("beta.txt", "file"), ("link", "symlink")]
    );
    assert_eq!((l.total, l.truncated, l.error), (5, false, None));

    let capped = list_dir(dir.to_str().unwrap(), 2);
    assert_eq!((capped.entries.len(), capped.total, capped.truncated), (2, 5, true));

    let missing = list_dir(dir.join("nope").to_str().unwrap(), 10);
    assert!(missing.error.is_some() && missing.entries.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ── classification ──────────────────────────────────────────────────────────

#[test]
fn classify_reports_reasons_ancestors_and_folders_that_contain_sources() {
    let built = build_rules(&input(&["/home/u/code"], &["node_modules"]), &no_files, &no_env);
    let engine = Engine::new(&built.rules).unwrap();
    let probe = FsProbe::default();
    let paths: Vec<String> =
        ["/home/u/code", "/home/u/code/app/node_modules/x.js", "/home/u", "/etc", "/home/u/code/app"].map(String::from).to_vec();
    let v = classify_paths(&engine, &probe, &paths);

    assert!(v[0].included);
    assert!(!v[1].included);
    assert_eq!(v[1].reason, Some(ReasonOut { kind: "pattern", rule_id: Some(1), spec: None }));
    assert_eq!(v[1].at.as_deref(), Some("/home/u/code/app/node_modules"));
    // An ancestor of a source is not itself backed up, but contains something that is.
    assert!(!v[2].included && v[2].contains_sources);
    assert_eq!(v[2].reason.as_ref().map(|r| r.kind), Some("outsideSources"));
    assert!(!v[3].included && !v[3].contains_sources);
    assert!(v[4].included);
}

// ── scanning ────────────────────────────────────────────────────────────────

const GENEROUS: ScanLimits = ScanLimits { budget: Duration::from_secs(30), max_entries: 10_000_000 };

fn engine_for(root: &std::path::Path, excludes: &[&str], markers: &[&str]) -> Engine {
    let mut i = input(&[root.to_str().unwrap()], excludes);
    i.exclude_if_present = markers.iter().map(|m| m.to_string()).collect();
    Engine::new(&build_rules(&i, &no_files, &no_env).rules).unwrap()
}

#[test]
fn a_scan_finds_the_shallowest_excluded_item_and_stops() {
    let root = scratch("partial");
    touch(&root.join("a/keep.txt"));
    touch(&root.join("a/deep/er/node_modules/x.js"));
    touch(&root.join("b/node_modules/y.js"));
    let engine = engine_for(&root, &["node_modules"], &[]);
    let cancel = AtomicBool::new(false);
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &cancel);
    assert!(o.partial && o.complete);
    // Breadth first: b/node_modules (depth 2) is found before a/deep/er/node_modules (depth 4).
    assert_eq!(o.first_excluded.as_deref(), Some(root.join("b/node_modules").to_str().unwrap()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scan_that_finds_nothing_reports_a_completely_included_folder() {
    let root = scratch("clean");
    touch(&root.join("a/keep.txt"));
    touch(&root.join("b/c/d.txt"));
    let engine = engine_for(&root, &["node_modules", "*.log"], &[]);
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(false));
    assert_eq!((o.partial, o.complete, o.unreadable), (false, true, false));
    assert!(o.scanned >= 4);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn with_no_rules_nothing_is_read_at_all() {
    let engine = Engine::new(&PlanRules { sources: vec!["/nonexistent".into()], ..Default::default() }).unwrap();
    let o = scan_dir(&engine, &FsProbe::default(), "/nonexistent", GENEROUS, &AtomicBool::new(false));
    assert_eq!((o.partial, o.complete, o.scanned), (false, true, 0));
}

#[test]
fn a_scan_does_not_enter_excluded_directories_or_follow_symlinks() {
    let root = scratch("skip");
    touch(&root.join("keep/ok.txt"));
    // Excluded directory holding a file the pattern would otherwise reject: never visited.
    touch(&root.join("vendor/inner/skip.log"));
    // A symlink to a directory containing an excluded item must not be followed.
    let outside = scratch("outside");
    touch(&outside.join("x.log"));
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let vendor_rule = format!("{}/vendor", root.display());
    let engine = engine_for(&root, &[vendor_rule.as_str(), "*.log"], &[]);
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(false));
    // vendor itself is rejected, so that *is* the first excluded item — and nothing past it.
    assert_eq!(o.first_excluded.as_deref(), Some(root.join("vendor").to_str().unwrap()));

    let engine = engine_for(&root, &["*.log"], &[]);
    let _ = std::fs::remove_dir_all(root.join("vendor"));
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(false));
    assert!(!o.partial, "the symlinked folder's x.log must not count: {o:?}");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn markers_make_the_siblings_of_the_marker_excluded() {
    let root = scratch("marker");
    touch(&root.join("proj/.nobackup"));
    touch(&root.join("proj/data.bin"));
    let engine = engine_for(&root, &[], &[".nobackup"]);
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(false));
    assert!(o.partial);
    assert_eq!(o.first_excluded.as_deref(), Some(root.join("proj/data.bin").to_str().unwrap()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_exhausted_budget_or_a_cancel_leaves_the_verdict_incomplete() {
    let root = scratch("budget");
    for i in 0..600 {
        touch(&root.join(format!("d{}/f{i}.txt", i % 10)));
    }
    let engine = engine_for(&root, &["*.log"], &[]);
    // Entry budget exhausted (checked every 256 entries).
    let tight = ScanLimits { budget: Duration::from_secs(30), max_entries: 100 };
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), tight, &AtomicBool::new(false));
    assert!(!o.complete && !o.partial, "{o:?}");
    // Cancelled.
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(true));
    assert!(!o.complete, "{o:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unreadable_folder_means_nothing_can_be_asserted() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("unreadable");
    touch(&root.join("open/a.txt"));
    touch(&root.join("locked/b.txt"));
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let engine = engine_for(&root, &["*.log"], &[]);
    let o = scan_dir(&engine, &FsProbe::default(), root.to_str().unwrap(), GENEROUS, &AtomicBool::new(false));
    std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    // Running as root can read it anyway; either way the outcome must be consistent.
    if o.unreadable {
        assert!(!o.complete && !o.partial);
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn kinds_reports_what_each_path_is() {
    let dir = scratch("kinds");
    touch(&dir.join("f.txt"));
    std::fs::create_dir_all(dir.join("d")).unwrap();
    std::os::unix::fs::symlink(dir.join("d"), dir.join("link")).unwrap();
    let paths: Vec<String> =
        ["f.txt", "d", "link", "nope"].iter().map(|n| dir.join(n).to_str().unwrap().to_string()).collect();
    assert_eq!(kinds(&paths, false), ["file", "dir", "symlink", "missing"]);
    // A symlink source is treated as its target.
    assert_eq!(kinds(&paths, true), ["file", "dir", "dir", "missing"]);
    let _ = std::fs::remove_dir_all(&dir);
}
