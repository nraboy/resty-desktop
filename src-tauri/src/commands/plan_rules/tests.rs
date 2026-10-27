use std::collections::HashSet;

use super::*;

use super::test_tables::{GO_MATCH, RESTIC_CHILD_MATCH, RESTIC_LIST, RESTIC_MATCH};

// ── ports agree with the originals ──────────────────────────────────────────

#[test]
fn go_match_agrees_with_gos_own_test_table() {
    for (pattern, name, want, want_err) in GO_MATCH {
        let got = go_match(pattern, name);
        if *want_err {
            assert_eq!(got, Err(BadPattern), "Match({pattern:?}, {name:?}) should be ErrBadPattern");
        } else {
            assert_eq!(got, Ok(*want), "Match({pattern:?}, {name:?})");
        }
    }
}

#[test]
fn pattern_matching_agrees_with_restics_match_table() {
    for (pattern, path, want) in RESTIC_MATCH {
        if path.is_empty() {
            continue;
        }
        assert_eq!(pattern_matches(pattern, path), Ok(*want), "Match({pattern:?}, {path:?})");
    }
}

#[test]
fn child_matching_agrees_with_restics_child_match_table() {
    for (pattern, path, want) in RESTIC_CHILD_MATCH {
        if path.is_empty() {
            continue;
        }
        assert_eq!(pattern_child_matches(pattern, path), Ok(*want), "ChildMatch({pattern:?}, {path:?})");
    }
}

#[test]
fn pattern_lists_agree_with_restics_list_table_with_and_without_the_index() {
    for (patterns, path, want_match, want_child) in RESTIC_LIST {
        let parsed: Vec<Pattern> =
            patterns.iter().filter(|p| !p.is_empty()).map(|p| Pattern::parse(p).unwrap()).collect();
        let list = PatternList::new(parsed, false);
        for (label, out) in [("indexed", list.evaluate(path)), ("naive", list.evaluate_naive(path))] {
            assert_eq!(out.matched, *want_match, "{label} List({patterns:?}, {path:?}) matched");
            assert_eq!(out.child_may_match, *want_child, "{label} List({patterns:?}, {path:?}) child");
        }
    }
}

#[test]
fn malformed_patterns_are_rejected_like_validate_patterns() {
    for bad in ["[", "a[", "[a-", "\\", "foo/[", "[]a]", "[-x]", "!["] {
        assert!(Pattern::parse(bad).is_err(), "{bad:?} should be invalid");
    }
    for ok in ["*.log", "node_modules/", "/a/b/**", "!/a", "[a-z]*", "a\\*b", "**"] {
        assert!(Pattern::parse(ok).is_ok(), "{ok:?} should be valid");
    }
}

#[test]
fn clean_path_matches_go() {
    for (input, want) in [
        ("", "."),
        ("/", "/"),
        ("/foo/bar/", "/foo/bar"),
        ("/foo///bar", "/foo/bar"),
        ("/foo/../bar", "/bar"),
        ("/../x", "/x"),
        ("a/./b", "a/b"),
        ("../a", "../a"),
        ("a/../../b", "../b"),
        ("node_modules/", "node_modules"),
    ] {
        assert_eq!(clean_path(input), want, "Clean({input:?})");
    }
}

// ── engine behaviour (each case was checked against restic 0.19.1 dry runs) ──

#[derive(Default)]
struct MockProbe {
    /// (directory, marker file name) pairs that "exist".
    markers: HashSet<(String, String)>,
}

impl Probe for MockProbe {
    fn has_marker(&self, dir: &str, marker: &Marker) -> bool {
        self.markers.contains(&(dir.to_string(), marker.name.clone()))
    }
}

fn rules(sources: &[&str], patterns: &[&str]) -> PlanRules {
    PlanRules {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        rules: patterns
            .iter()
            .enumerate()
            .map(|(i, p)| RuleInput { id: i as u32 + 1, pattern: p.to_string(), ignore_case: false })
            .collect(),
        markers: vec![],
        exclude_caches: false,
        style: PathStyle::Unix,
        ..Default::default()
    }
}

fn included(e: &Engine, path: &str) -> bool {
    e.classify(path, &MockProbe::default()).included
}

#[test]
fn nothing_is_excluded_without_rules() {
    let e = Engine::new(&rules(&["/s"], &[])).unwrap();
    for p in ["/s", "/s/a", "/s/a/node_modules/x.js", "/s/.env"] {
        assert!(included(&e, p), "{p}");
    }
}

#[test]
fn an_unanchored_name_excludes_it_at_any_depth_and_everything_below() {
    let e = Engine::new(&rules(&["/s"], &["node_modules"])).unwrap();
    assert!(included(&e, "/s/a/keep.txt"));
    assert!(!included(&e, "/s/a/node_modules"));
    assert!(!included(&e, "/s/c/deep/node_modules"));
    // Children of an excluded directory are excluded because their ancestor is.
    let v = e.classify("/s/a/node_modules/x.js", &MockProbe::default());
    assert!(!v.included);
    assert_eq!(v.reason, Some(Reason::Pattern { rule_id: 1 }));
    assert_eq!(v.at.as_deref(), Some("/s/a/node_modules"));
}

#[test]
fn wildcards_and_extensions() {
    let e = Engine::new(&rules(&["/s"], &["*.log"])).unwrap();
    assert!(!included(&e, "/s/top.log"));
    assert!(!included(&e, "/s/e/sub/f.log"));
    assert!(included(&e, "/s/e/sub/g.txt"));
}

#[test]
fn an_absolute_pattern_excludes_the_path_and_its_whole_subtree() {
    let e = Engine::new(&rules(&["/s"], &["/s/a"])).unwrap();
    assert!(!included(&e, "/s/a"));
    assert!(!included(&e, "/s/a/node_modules/x.js"));
    assert!(included(&e, "/s/b/ok.txt"));
}

#[test]
fn the_source_root_is_exempt_but_its_contents_are_not() {
    let e = Engine::new(&rules(&["/s"], &["/s"])).unwrap();
    assert!(included(&e, "/s"));
    assert!(!included(&e, "/s/a"));
}

#[test]
fn a_folder_on_the_way_to_another_source_is_walked_through_even_when_a_rule_rejects_it() {
    let mut r = rules(&["/s", "/s/a/b/c"], &[]);
    r.markers = vec![".nobackup".to_string()];
    let mut probe = MockProbe::default();
    probe.markers.insert(("/s".to_string(), ".nobackup".to_string())); // rejects its sibling `a`
    let e = Engine::new(&r).unwrap();
    // `a` leads to the source `/s/a/b/c`, so it is not rejected and its other children are judged alone.
    assert!(e.classify("/s/a", &probe).included);
    assert!(e.classify("/s/a/other.txt", &probe).included);
    assert!(e.classify("/s/a/b/c/f", &probe).included);
    // A sibling that leads nowhere is still dropped by the marker.
    assert!(!e.classify("/s/z", &probe).included);
    // The same on a restic before 0.19 (checked against 0.17.3), whose only difference is that it
    // also judges the sources themselves.
    r.judge_sources = true;
    let old = Engine::new(&r).unwrap();
    assert!(old.classify("/s/a/other.txt", &probe).included);
    assert!(!old.classify("/s/z", &probe).included);
}

#[test]
fn a_source_holding_another_source_is_never_judged_and_its_folders_to_it_are_always_kept() {
    // `s` matches the rule (it is a component of every path below), so every path here is rejected
    // by it unless it is exempt.
    for judge in [false, true] {
        let e = Engine::new(&PlanRules { judge_sources: judge, ..rules(&["/s", "/s/a/b"], &["s"]) }).unwrap();
        let probe = MockProbe::default();
        // `/s` holds `/s/a/b`, so it is a folder of the target tree, not a judged leaf; `/s/a` leads to `b`.
        assert!(included(&e, "/s") && included(&e, "/s/a"), "judge_sources={judge}");
        // Their other children are judged one by one, and fail the rule.
        assert!(!included(&e, "/s/other"), "judge_sources={judge}");
        assert!(!e.classify("/s/a/other", &probe).included, "judge_sources={judge}");
        // The leaf source: exempt from 0.19, judged (and dropped) before.
        assert_eq!(included(&e, "/s/a/b"), !judge, "judge_sources={judge}");
    }
}

// ── restic before 0.19 judges the source paths themselves (restic #5767) ─────

/// The same plan as `rules`, for a restic older than 0.19.
fn old_restic(sources: &[&str], patterns: &[&str]) -> PlanRules {
    PlanRules { judge_sources: true, ..rules(sources, patterns) }
}

#[test]
fn before_0_19_a_source_that_matches_an_exclude_is_dropped_with_everything_in_it() {
    // 0.19+: the source is exempt, its contents are judged.
    let new = Engine::new(&rules(&["/s/node_modules"], &["node_modules"])).unwrap();
    assert!(included(&new, "/s/node_modules"));
    // Before: the source itself is excluded by the pattern, so nothing under it is reached.
    let old = Engine::new(&old_restic(&["/s/node_modules"], &["node_modules"])).unwrap();
    let v = old.classify("/s/node_modules", &MockProbe::default());
    assert!(!v.included);
    assert_eq!(v.reason, Some(Reason::Pattern { rule_id: 1 }));
    assert_eq!(v.at, None, "the queried path is the rejected source itself");
    let inside = old.classify("/s/node_modules/x.js", &MockProbe::default());
    assert!(!inside.included);
    assert_eq!(inside.at.as_deref(), Some("/s/node_modules"));
}

#[test]
fn before_0_19_a_source_passing_every_rule_is_still_included() {
    let old = Engine::new(&old_restic(&["/s", "/t/keep.txt"], &["*.log", "node_modules"])).unwrap();
    for p in ["/s", "/s/a/b.txt", "/t/keep.txt"] {
        assert!(included(&old, p), "{p}");
    }
    assert!(!included(&old, "/s/a/x.log"));
    // No rules at all: nothing to judge.
    let none = Engine::new(&old_restic(&["/s"], &[])).unwrap();
    assert!(included(&none, "/s") && included(&none, "/s/a/b"));
}

#[test]
fn before_0_19_an_explicit_file_matching_a_pattern_is_excluded_but_not_a_sibling_source() {
    let new = Engine::new(&rules(&["/s", "/s/a/keep.txt"], &["*.txt"])).unwrap();
    assert!(included(&new, "/s/a/keep.txt"), "0.19: named paths are exempt");
    let old = Engine::new(&old_restic(&["/s/a/keep.txt", "/s/b/other.log"], &["*.txt"])).unwrap();
    assert!(!included(&old, "/s/a/keep.txt"), "before 0.19 it is excluded like any file");
    assert!(included(&old, "/s/b/other.log"), "the other source passes the rule");
}

#[test]
fn before_0_19_a_marker_beside_a_source_drops_it() {
    let mut r = rules(&["/s/proj"], &[]);
    r.markers = vec![".nobackup".to_string()];
    let mut probe = MockProbe::default();
    probe.markers.insert(("/s".to_string(), ".nobackup".to_string()));
    // 0.19+: `/s/proj` was named on the command line, so the marker next to it is ignored.
    assert!(Engine::new(&r).unwrap().classify("/s/proj", &probe).included);
    // Before: `/s/proj` is a sibling of the marker like any other item of `/s`.
    r.judge_sources = true;
    let v = Engine::new(&r).unwrap().classify("/s/proj", &probe);
    assert!(!v.included);
    assert_eq!(v.reason, Some(Reason::Marker { spec: ".nobackup".to_string() }));
}

#[test]
fn before_0_19_a_path_under_several_sources_is_included_if_any_source_survives() {
    // `/s/a` is a source dropped by the pattern, but `/s` itself survives and holds `/s/a/f`... only
    // if `a` doesn't match: here it does, so the contents are still rejected by the parent source.
    let old = Engine::new(&old_restic(&["/s", "/s/a"], &["a"])).unwrap();
    assert!(!included(&old, "/s/a"));
    assert!(!included(&old, "/s/a/f"));
    // An unrelated name pattern leaves both sources standing.
    let old = Engine::new(&old_restic(&["/s", "/s/a"], &["zzz"])).unwrap();
    assert!(included(&old, "/s/a") && included(&old, "/s/a/f"));
}

#[test]
fn all_but_one_with_a_later_negation() {
    let e = Engine::new(&rules(&["/s"], &["/s/*", "!/s/a"])).unwrap();
    assert!(included(&e, "/s/a"));
    assert!(included(&e, "/s/a/keep.txt"));
    assert!(included(&e, "/s/a/node_modules/x.js"));
    assert!(!included(&e, "/s/b"));
    // The negation is credited for re-including /s/a.
    assert_eq!(e.classify("/s/a", &MockProbe::default()).reincluded_by, Some(2));
}

#[test]
fn order_matters_a_negation_before_the_exclude_does_nothing() {
    let e = Engine::new(&rules(&["/s"], &["!/s/a", "/s/*"])).unwrap();
    assert!(!included(&e, "/s/a"));
}

#[test]
fn nothing_inside_an_excluded_directory_can_be_reincluded() {
    let e = Engine::new(&rules(&["/s"], &["/s/a", "!/s/a/keep.txt"])).unwrap();
    assert!(!included(&e, "/s/a/keep.txt"));
    // Same for a name pattern: node_modules stays out even with a matching negation.
    let e = Engine::new(&rules(&["/s"], &["node_modules", "!**/y.js"])).unwrap();
    assert!(!included(&e, "/s/c/deep/node_modules/y.js"));
}

#[test]
fn excluding_a_directorys_contents_instead_lets_one_child_through() {
    let e = Engine::new(&rules(&["/s"], &["/s/a/*", "!/s/a/keep.txt"])).unwrap();
    assert!(included(&e, "/s/a"));
    assert!(included(&e, "/s/a/keep.txt"));
    assert!(!included(&e, "/s/a/node_modules"));
}

#[test]
fn a_negated_directory_comes_back_with_its_whole_subtree() {
    let e = Engine::new(&rules(&["/s"], &["node_modules", "!/s/c/deep/node_modules"])).unwrap();
    assert!(included(&e, "/s/c/deep/node_modules"));
    assert!(included(&e, "/s/c/deep/node_modules/y.js"));
    assert!(!included(&e, "/s/a/node_modules"));
}

#[test]
fn the_gitignore_style_carve_out_chain_includes_exactly_one_file() {
    let e = Engine::new(&rules(
        &["/s"],
        &[
            "node_modules",
            "!/s/c/deep/node_modules",
            "/s/c/deep/node_modules/*",
            "!/s/c/deep/node_modules/y.js",
        ],
    ))
    .unwrap();
    assert!(included(&e, "/s/c/deep/node_modules/y.js"));
    assert!(!included(&e, "/s/c/deep/node_modules/other.js"));
    assert!(!included(&e, "/s/a/node_modules/x.js"));
}

#[test]
fn an_explicit_nested_source_is_exempt_itself() {
    let e = Engine::new(&rules(&["/s", "/s/a/keep.txt"], &["/s/a"])).unwrap();
    assert!(included(&e, "/s/a/keep.txt"));
    assert!(!included(&e, "/s/a/other.txt"));
    // ...but its contents are still judged: a directory source does not escape a prefix rule.
    let e = Engine::new(&rules(&["/s", "/s/a/node_modules"], &["/s/a"])).unwrap();
    assert!(included(&e, "/s/a/node_modules"));
    assert!(!included(&e, "/s/a/node_modules/x.js"));
}

#[test]
fn paths_outside_every_source_are_excluded_with_that_reason() {
    let e = Engine::new(&rules(&["/s"], &[])).unwrap();
    let v = e.classify("/elsewhere/x", &MockProbe::default());
    assert!(!v.included);
    assert_eq!(v.reason, Some(Reason::OutsideSources));
    // A sibling whose name merely starts like the source is outside it too.
    assert!(!included(&e, "/s2/x"));
}

#[test]
fn a_path_under_several_sources_is_included_if_any_includes_it() {
    let e = Engine::new(&rules(&["/s/a", "/s"], &["/s/a"])).unwrap();
    // Under source /s/a the root is exempt, and /s/a/x is excluded by the prefix rule; but
    // under source /s, /s/a is itself rejected — so nothing below it is backed up.
    assert!(included(&e, "/s/a"));
    assert!(!included(&e, "/s/a/x"));
}

#[test]
fn exclude_if_present_keeps_the_directory_and_the_marker_but_drops_the_rest() {
    let mut r = rules(&["/s"], &[]);
    r.markers = vec![".nobackup".to_string()];
    let e = Engine::new(&r).unwrap();
    let mut probe = MockProbe::default();
    probe.markers.insert(("/s/b".to_string(), ".nobackup".to_string()));
    assert!(e.classify("/s/b", &probe).included);
    assert!(e.classify("/s/b/.nobackup", &probe).included);
    let v = e.classify("/s/b/ok.txt", &probe);
    assert!(!v.included);
    assert_eq!(v.reason, Some(Reason::Marker { spec: ".nobackup".to_string() }));
    assert!(!e.classify("/s/b/sub/deeper.txt", &probe).included);
    assert!(e.classify("/s/a/ok.txt", &probe).included);
}

#[test]
fn case_insensitive_rules_are_their_own_list() {
    let mut r = rules(&["/s"], &["Node_Modules"]);
    r.rules[0].ignore_case = true;
    let e = Engine::new(&r).unwrap();
    assert!(!included(&e, "/s/c/deep/node_modules"));

    // A case-sensitive `!` does not cancel a case-insensitive exclude.
    let mut r = rules(&["/s"], &["NODE_MODULES", "!/s/c/deep/node_modules"]);
    r.rules[0].ignore_case = true;
    let e = Engine::new(&r).unwrap();
    assert!(!included(&e, "/s/c/deep/node_modules"));
}

#[test]
fn exclude_caches_uses_the_cachedir_tag_signature() {
    let dir = scratch_dir("caches");
    let cache = dir.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("data"), "x").unwrap();
    let fake = dir.join("fake");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::write(fake.join("data"), "x").unwrap();

    std::fs::write(cache.join("CACHEDIR.TAG"), format!("{}\n# rest", &CACHEDIR_TAG_SPEC[CACHEDIR_TAG_SPEC.find(':').unwrap() + 1..])).unwrap();
    std::fs::write(fake.join("CACHEDIR.TAG"), "not the signature").unwrap();

    // Real paths in the engine's own spelling (`/` separators, `C:/…` on Windows) and the host's rules.
    let path = |p: &std::path::Path| canon(p.to_str().unwrap());
    let mut r = rules(&[path(&dir).as_str()], &[]);
    r.style = PathStyle::NATIVE;
    r.exclude_caches = true;
    let e = Engine::new(&r).unwrap();
    let probe = FsProbe::default();
    assert!(!e.classify(&path(&cache.join("data")), &probe).included);
    assert!(e.classify(&path(&cache.join("CACHEDIR.TAG")), &probe).included);
    assert!(e.classify(&path(&fake.join("data")), &probe).included);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_patterns_and_markers_are_reported() {
    assert!(Engine::new(&rules(&["/s"], &["[oops"])).is_err());
    let mut r = rules(&["/s"], &[]);
    r.markers = vec![":header-only".to_string()];
    assert!(Engine::new(&r).is_err());
    assert!(Marker::parse("").is_err());
    assert_eq!(Marker::parse("CACHEDIR.TAG:Sig").unwrap().header, "Sig");
}

// ── the speed index never changes an answer ─────────────────────────────────

fn scratch_dir(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "resty-plan-rules-{}-{label}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const NAMES: &[&str] = &["a", "b", "c", "src", "deep", "node_modules", ".env", ".venv", "x.log", "keep.txt", "Cap"];

fn random_pattern(rng: &mut rand::rngs::StdRng, root: &str) -> String {
    use rand::Rng;
    let n = |rng: &mut rand::rngs::StdRng| NAMES[rng.gen_range(0..NAMES.len())];
    let body = match rng.gen_range(0..11) {
        0 => n(rng).to_string(),
        1 => format!("*.{}", ["log", "txt", "go"][rng.gen_range(0..3)]),
        2 => format!("{root}/{}", n(rng)),
        3 => format!("{root}/{}/*", n(rng)),
        4 => format!("{}/{}", n(rng), n(rng)),
        5 => format!("**/{}", n(rng)),
        6 => format!("{root}/**/{}", n(rng)),
        7 => format!("{}*", &n(rng)[..1]),
        8 => format!("**/{}/**/{}", n(rng), n(rng)),
        9 => format!("{root}/**/{}/**", n(rng)),
        _ => format!("{root}/{}/{}", n(rng), n(rng)),
    };
    if rng.gen_range(0..4) == 0 { format!("!{body}") } else { body }
}

fn random_path(rng: &mut rand::rngs::StdRng, root: &str) -> String {
    use rand::Rng;
    let depth = rng.gen_range(1..=5);
    let mut p = root.to_string();
    for _ in 0..depth {
        p.push('/');
        p.push_str(NAMES[rng.gen_range(0..NAMES.len())]);
    }
    p
}

#[test]
fn the_index_gives_the_same_answer_as_trying_every_pattern() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xC0FFEE);
    for _ in 0..400 {
        let n = rng.gen_range(1..8);
        let ignore_case = rng.gen_range(0..4) == 0;
        let patterns: Vec<Pattern> = (0..n)
            .map(|_| {
                let p = random_pattern(&mut rng, "/r");
                Pattern::parse(&if ignore_case { p.to_lowercase() } else { p }).unwrap()
            })
            .collect();
        let list = PatternList::new(patterns, ignore_case);
        for _ in 0..40 {
            let path = random_path(&mut rng, "/r");
            assert_eq!(list.evaluate(&path), list.evaluate_naive(&path), "path {path}");
        }
    }
}

#[test]
fn the_allocation_free_match_agrees_with_the_literal_port() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xFEED);
    let words = ["a", "b", "src", "node_modules", "x.log", "*", "?", "[a-c]*", "*.log", "**", "**", "**"];
    for _ in 0..20_000 {
        let mut pattern = String::new();
        if rng.gen_range(0..3) == 0 {
            pattern.push('/');
        }
        for i in 0..rng.gen_range(1..=5) {
            if i > 0 {
                pattern.push('/');
            }
            pattern.push_str(words[rng.gen_range(0..words.len())]);
        }
        let pat = match Pattern::parse(&pattern) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let mut path = String::new();
        if rng.gen_range(0..4) != 0 {
            path.push('/');
        }
        let n = rng.gen_range(1..=6);
        for i in 0..n {
            if i > 0 || !path.is_empty() && !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(["a", "b", "c", "src", "node_modules", "x.log", "d.log", "ab"][rng.gen_range(0..8)]);
        }
        let strs = split_path(&path);
        assert_eq!(
            match_parts(&pat.parts, &strs),
            match_parts_reference(&pat.parts, &strs),
            "pattern {pattern:?} vs path {path:?}"
        );
    }
}

// ── against the real restic ─────────────────────────────────────────────────

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

fn restic_available() -> bool {
    std::process::Command::new("restic")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn make_tree(rng: &mut rand::rngs::StdRng, root: &std::path::Path) -> Vec<std::path::PathBuf> {
    use rand::Rng;
    let mut all = Vec::new();
    fn grow(rng: &mut rand::rngs::StdRng, dir: &std::path::Path, depth: usize, all: &mut Vec<std::path::PathBuf>) {
        use rand::Rng;
        let count = rng.gen_range(1..=4);
        for _ in 0..count {
            let name = NAMES[rng.gen_range(0..NAMES.len())];
            let path = dir.join(name);
            if path.exists() {
                continue;
            }
            // Names that read as files stay files; others are directories most of the time.
            let is_file = name.contains('.') && name != ".venv" && name != ".env"
                || depth >= 3
                || rng.gen_range(0..4) == 0;
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
    let _ = rng.gen_range(0..1);
    grow(rng, root, 0, &mut all);
    all
}

/// The set of absolute paths restic reports for a dry run of `backup <sources>` with `args`.
fn restic_paths(repo: &std::path::Path, sources: &[std::path::PathBuf], args: &[String]) -> HashSet<String> {
    let out = std::process::Command::new("restic")
        .args(["backup", "--repo"])
        .arg(repo)
        .args(["--dry-run", "-vv", "--json", "--no-scan"])
        .args(args)
        .args(sources)
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

/// Compares the engine with restic itself on random trees and rule sets. Needs `restic` on
/// PATH; run with `cargo test restic_agrees -- --ignored --nocapture`.
#[test]
#[ignore]
fn restic_agrees_with_the_engine_on_random_trees_and_rules() {
    use rand::{Rng, SeedableRng};
    if !restic_available() {
        eprintln!("restic not found on PATH; skipping");
        return;
    }
    let base = scratch_dir("diff");
    let repo = base.join("repo");
    assert!(std::process::Command::new("restic")
        .args(["init", "--repo"])
        .arg(&repo)
        .env("RESTIC_PASSWORD", "pw")
        .output()
        .unwrap()
        .status
        .success());

    let cases: u64 = std::env::var("RESTY_DIFF_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let seed: u64 = std::env::var("RESTY_DIFF_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(7);
    let mut checked = 0usize;
    let (mut n_included, mut n_excluded, mut n_negated_cases, mut n_marker_cases, mut n_ci_cases) = (0, 0, 0, 0, 0);
    let mut n_extra_source_cases = 0usize;
    for case in 0..cases {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed * 1000 + case);
        let root = base.join(format!("tree{case}"));
        std::fs::create_dir_all(&root).unwrap();
        let root = plain_canonical(&root);
        let nodes = make_tree(&mut rng, &root);
        let root_str = canon(root.to_str().unwrap());

        let n_rules = rng.gen_range(1..6);
        let mut rule_inputs = Vec::new();
        let mut args = Vec::new();
        for i in 0..n_rules {
            let pattern = random_pattern(&mut rng, &root_str);
            let ignore_case = rng.gen_range(0..6) == 0;
            let pattern = if ignore_case { pattern.to_lowercase() } else { pattern };
            args.push(if ignore_case { "--iexclude" } else { "--exclude" }.to_string());
            args.push(pattern.clone());
            rule_inputs.push(RuleInput { id: i + 1, pattern, ignore_case });
        }
        let mut markers = Vec::new();
        if rng.gen_range(0..4) == 0 {
            // A marker dropped into one directory of the tree.
            if let Some(dir) = nodes.iter().find(|p| p.is_dir()) {
                std::fs::write(dir.join(".nobackup"), "x").unwrap();
                markers.push(".nobackup".to_string());
                args.push("--exclude-if-present".to_string());
                args.push(".nobackup".to_string());
            }
        }

        n_negated_cases += usize::from(rule_inputs.iter().any(|r| r.pattern.starts_with('!')));
        n_marker_cases += usize::from(!markers.is_empty());
        n_ci_cases += usize::from(rule_inputs.iter().any(|r| r.ignore_case));
        // The tree's root, and half the time one or two more paths named explicitly — a folder or a
        // file somewhere inside it — which is where restic's rules for *source paths* show: from
        // 0.19 they are exempt from the excludes, before that they are judged like anything else.
        let mut sources = vec![root.clone()];
        if rng.gen_range(0..2) == 0 {
            for _ in 0..rng.gen_range(1..=2) {
                let extra = nodes[rng.gen_range(0..nodes.len())].clone();
                if !sources.contains(&extra) {
                    sources.push(extra);
                }
            }
        }
        n_extra_source_cases += usize::from(sources.len() > 1);
        let judge = crate::commands::test_support::restic_judges_sources();
        let engine = Engine::new(&PlanRules {
            sources: sources.iter().map(|s| canon(s.to_str().unwrap())).collect(),
            rules: rule_inputs.clone(),
            markers,
            exclude_caches: false,
            style: PathStyle::NATIVE,
            judge_sources: judge,
            ..Default::default()
        })
        .unwrap();
        let probe = FsProbe::default();
        let reported = restic_paths(&repo, &sources, &args);

        let mut all_nodes = nodes.clone();
        all_nodes.push(root.join(".nobackup"));
        for node in all_nodes.iter().filter(|p| p.exists()) {
            let path = canon(node.to_str().unwrap());
            let path = path.as_str();
            let want = reported.contains(path);
            let got = engine.classify(path, &probe);
            assert_eq!(
                got.included, want,
                "case {case} (seed {seed}): {path}\nsources: {sources:?}\nrules: {args:?}\nengine said {got:?}, restic {}",
                if want { "included it" } else { "excluded it" }
            );
            checked += 1;
            if want { n_included += 1 } else { n_excluded += 1 }
        }
    }
    eprintln!(
        "engine and restic agree on {checked} nodes over {cases} random cases \
         ({n_included} included, {n_excluded} excluded; cases with a `!`: {n_negated_cases}, a marker: {n_marker_cases}, an --iexclude: {n_ci_cases}, extra explicit sources: {n_extra_source_cases}; restic {} source paths)",
        if crate::commands::test_support::restic_judges_sources() { "judges" } else { "exempts" }
    );
    let _ = std::fs::remove_dir_all(&base);
}

// ── speed ───────────────────────────────────────────────────────────────────

/// Rough numbers for the rule engine on a synthetic home-directory-sized workload. Run with
/// `cargo test engine_speed --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn engine_speed() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(1);
    let patterns = [
        "node_modules", ".venv", ".git", "*.log", ".DS_Store", "__pycache__", "*.tmp", "target",
        "/r/Library/Caches", "/r/Downloads/*", "!/r/Downloads/keep", "**/build", "dist/cache",
    ];
    let rule_inputs: Vec<RuleInput> = patterns
        .iter()
        .enumerate()
        .map(|(i, p)| RuleInput { id: i as u32, pattern: p.to_string(), ignore_case: false })
        .collect();
    let engine = Engine::new(&PlanRules {
        sources: vec!["/r".to_string()],
        rules: rule_inputs,
        markers: vec![],
        exclude_caches: false,
        style: PathStyle::Unix,
        ..Default::default()
    })
    .unwrap();
    let probe = MockProbe::default();
    let paths: Vec<String> = (0..1_000_000)
        .map(|_| {
            let depth = rng.gen_range(2..9);
            let mut p = "/r".to_string();
            for _ in 0..depth {
                p.push_str(&format!("/d{}", rng.gen_range(0..400)));
            }
            p
        })
        .collect();
    for (label, hierarchical) in [("judge_node (one node, parent known included)", false), ("classify (re-judges every ancestor)", true)] {
        let start = std::time::Instant::now();
        let mut excluded = 0usize;
        for p in &paths {
            let out = if hierarchical { !engine.classify(p, &probe).included } else { engine.judge_node(p, &probe).is_some() };
            excluded += usize::from(out);
        }
        let elapsed = start.elapsed();
        eprintln!(
            "{label}: {} paths, {} rules, {:?} — {:.0} ns/path, {excluded} excluded",
            paths.len(),
            patterns.len(),
            elapsed,
            elapsed.as_nanos() as f64 / paths.len() as f64
        );
    }
}

// ── Windows path rules ──────────────────────────────────────────────────────
//
// restic converts every Windows path and pattern to `/`-separated components before matching
// (`filepath.ToSlash`), so the matcher is shared; what differs is the path handling around it,
// tested here as pure string logic so it runs on every platform. The end-to-end check against
// real restic on Windows is the compatibility workflow (.github/workflows/restic-compat.yml).

#[test]
fn windows_clean_is_filepath_clean_then_to_slash() {
    for (input, want) in [
        ("", "."),
        ("C:\\", "C:/"),
        ("C:/", "C:/"),
        ("C:", "C:."),
        ("C:\\Users\\me\\", "C:/Users/me"),
        ("C:\\Users\\\\me", "C:/Users/me"),
        ("c:/a/../b", "c:/b"),
        ("C:\\..\\x", "C:/x"),
        ("C:\\a\\.\\b", "C:/a/b"),
        ("\\foo\\bar", "/foo/bar"),
        ("/foo/bar/", "/foo/bar"),
        ("a\\b", "a/b"),
        ("..\\a", "../a"),
        ("C:foo", "C:foo"),
    ] {
        assert_eq!(PathStyle::Windows.clean(input), want, "Clean({input:?})");
    }
    // Unix keeps a backslash as an ordinary character.
    assert_eq!(PathStyle::Unix.clean("a\\b/c"), "a\\b/c");
}

#[test]
fn path_styles_join_split_and_spell_paths() {
    let (u, w) = (PathStyle::Unix, PathStyle::Windows);
    assert_eq!(u.join("/", "etc"), "/etc");
    assert_eq!(u.join("/etc", "hosts"), "/etc/hosts");
    assert_eq!(w.join("C:/", "Users"), "C:/Users");
    assert_eq!(w.join("C:/Users", "me"), "C:/Users/me");

    assert_eq!(u.parent("/etc"), "/");
    assert_eq!(u.parent("/etc/hosts"), "/etc");
    assert_eq!(w.parent("C:/Users"), "C:/");
    assert_eq!(w.parent("C:/Users/me"), "C:/Users");
    assert_eq!(w.parent("C:/"), "C:/");

    assert_eq!(w.to_native("C:/Users/me"), "C:\\Users\\me");
    assert_eq!(u.to_native("/etc/hosts"), "/etc/hosts");

    assert_eq!(strip_source("C:/Users/me/x", "C:/Users/me"), Some("x"));
    assert_eq!(strip_source("C:/Users/me", "C:/Users/me"), Some(""));
    assert_eq!(strip_source("C:/Users/meow", "C:/Users/me"), None);
    assert_eq!(strip_source("C:/Users", "C:/"), Some("Users"));
    assert_eq!(strip_source("D:/x", "C:/"), None);
    assert_eq!(strip_source("/etc/x", "/"), Some("etc/x"));
}

#[test]
fn windows_escapes_a_bracket_with_a_class_because_backslash_is_a_separator() {
    assert_eq!(PathStyle::Windows.escape_glob("C:/a/we[ird]"), "C:/a/we[[]ird]");
    assert_eq!(PathStyle::Unix.escape_glob("/a/we[ird]"), "/a/we\\[ird\\]");
    // The escaped Windows pattern really matches the literal name and nothing else.
    let w = PathStyle::Windows;
    let pat = w.escape_glob("C:/s/we[ird]");
    assert_eq!(pattern_matches_styled(&pat, "C:/s/we[ird]/x", w), Ok(true));
    assert_eq!(pattern_matches_styled(&pat, "C:/s/weird/x", w), Ok(false));
}

#[test]
fn restic_pattern_tables_hold_under_windows_rules_too() {
    // restic's own filter tests run on its Windows CI, so every row must hold there as well.
    let w = PathStyle::Windows;
    for (pattern, path, want) in RESTIC_MATCH {
        if path.is_empty() {
            continue;
        }
        assert_eq!(pattern_matches_styled(pattern, path, w), Ok(*want), "windows Match({pattern:?}, {path:?})");
    }
    for (pattern, path, want) in RESTIC_CHILD_MATCH {
        if path.is_empty() {
            continue;
        }
        assert_eq!(pattern_child_matches_styled(pattern, path, w), Ok(*want), "windows ChildMatch({pattern:?}, {path:?})");
    }
    for (patterns, path, want_match, want_child) in RESTIC_LIST {
        let parsed: Vec<Pattern> =
            patterns.iter().filter(|p| !p.is_empty()).map(|p| Pattern::parse_styled(p, w).unwrap()).collect();
        let list = PatternList::new(parsed, false);
        let out = list.evaluate(path);
        assert_eq!((out.matched, out.child_may_match), (*want_match, *want_child), "windows List({patterns:?}, {path:?})");
    }
}

fn win_rules(sources: &[&str], patterns: &[&str]) -> PlanRules {
    PlanRules { style: PathStyle::Windows, ..rules(sources, patterns) }
}

#[test]
fn a_windows_pattern_may_use_either_separator_and_any_spelling_of_the_path() {
    let e = Engine::new(&win_rules(&["C:\\Users\\me"], &["node_modules", "C:\\Users\\me\\build", "*.log"])).unwrap();
    // The same path in both spellings gives the same answer.
    for p in ["C:\\Users\\me\\a\\node_modules\\x.js", "C:/Users/me/a/node_modules/x.js"] {
        assert!(!included(&e, p), "{p}");
    }
    assert!(!included(&e, "C:\\Users\\me\\build\\out.o"));
    assert!(!included(&e, "C:/Users/me/x.log"));
    assert!(included(&e, "C:\\Users\\me\\src\\main.rs"));
    // The source root itself is exempt; things outside every source are outside.
    assert!(included(&e, "C:\\Users\\me"));
    let v = e.classify("D:\\other\\x", &MockProbe::default());
    assert_eq!(v.reason, Some(Reason::OutsideSources));
    assert!(!included(&e, "C:\\Users\\meow\\x"));
}

#[test]
fn windows_re_includes_and_carve_outs_follow_the_same_ordering_rules() {
    let e = Engine::new(&win_rules(
        &["C:\\s"],
        &["C:\\s\\*", "!C:\\s\\a", "C:\\s\\a\\*", "!C:\\s\\a\\keep.txt"],
    ))
    .unwrap();
    assert!(included(&e, "C:/s/a"));
    assert!(included(&e, "C:/s/a/keep.txt"));
    assert!(!included(&e, "C:/s/a/other.txt"));
    assert!(!included(&e, "C:/s/b"));
}

#[test]
fn a_drive_root_can_be_a_source_and_markers_use_the_drive_aware_parent() {
    let mut r = win_rules(&["D:\\"], &[]);
    r.markers = vec![".nobackup".to_string()];
    let e = Engine::new(&r).unwrap();
    let mut probe = MockProbe::default();
    probe.markers.insert(("D:/".to_string(), ".nobackup".to_string()));
    probe.markers.insert(("D:/data".to_string(), ".nobackup".to_string()));
    // A marker in the drive root excludes its direct children (and so everything below them).
    assert!(!e.classify("D:/data", &probe).included);
    assert!(!e.classify("D:/data/x", &probe).included);
    assert!(e.classify("D:/", &probe).included);
    assert!(e.classify("D:/.nobackup", &probe).included);
}

#[test]
fn windows_case_insensitive_rules_ignore_case_in_the_whole_path() {
    let mut r = win_rules(&["C:\\Users\\me"], &["THUMBS.DB"]);
    r.rules[0].ignore_case = true;
    let e = Engine::new(&r).unwrap();
    assert!(!included(&e, "C:/Users/me/Pictures/thumbs.db"));
    assert!(included(&e, "C:/Users/me/Pictures/photo.jpg"));
}

#[test]
fn windows_rejects_an_invalid_pattern_like_restic() {
    assert!(Engine::new(&win_rules(&["C:\\s"], &["[oops"])).is_err());
    assert!(Pattern::parse_styled("we[ird", PathStyle::Windows).is_err());
}
