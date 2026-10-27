use std::collections::HashSet;

use super::*;

fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "resty-listfiles-{}-{label}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    plain_canonical(&dir)
}

/// `canonicalize` without the `\\?\` prefix Windows adds.
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

/// The `/` spelling of a native path.
fn canon(s: &str) -> String {
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.to_string()
    }
}

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x").unwrap();
}

fn style() -> PathStyle {
    PathStyle::NATIVE
}

/// A read function over in-memory list files.
fn files<'a>(list: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> std::io::Result<String> + 'a {
    move |p: &str| {
        list.iter()
            .find(|(name, _)| *name == p)
            .map(|(_, text)| text.to_string())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    }
}

fn paths(r: &ListFiles) -> Vec<String> {
    r.entries.iter().map(|e| e.path.clone()).collect()
}

// ── parsing ─────────────────────────────────────────────────────────────────

#[test]
fn lines_are_trimmed_and_comments_and_blanks_skipped_even_when_indented() {
    let text = "\u{feff}# header\n\n  /a/b  \r\n\t/c\t\n   # indented comment\n   \n/d#not-a-comment\n";
    assert_eq!(
        parse_lines(text),
        vec![(3, "/a/b".to_string()), (4, "/c".to_string()), (7, "/d#not-a-comment".to_string())]
    );
    assert!(parse_lines("").is_empty());
    assert!(parse_lines("\n\n# only\n").is_empty());
}

#[test]
fn split_and_clean_follow_gos_glob_helpers() {
    assert_eq!(split("/a/b/*.txt"), ("/a/b/", "*.txt"));
    assert_eq!(split("/*"), ("/", "*"));
    assert_eq!(split("*"), ("", "*"));
    assert_eq!(clean_glob_dir("/a/b/"), "/a/b");
    assert_eq!(clean_glob_dir("/"), "/");
    assert_eq!(clean_glob_dir("C:/"), "C:/");
    assert_eq!(clean_glob_dir("C:/a/"), "C:/a");
    assert_eq!(clean_glob_dir(""), ".");
}

#[test]
fn meta_characters_follow_the_platform() {
    assert!(has_meta("/a/*", PathStyle::Unix));
    assert!(has_meta("/a/[x]", PathStyle::Windows));
    assert!(has_meta("/a\\*b", PathStyle::Unix));
    // A backslash only escapes on Unix.
    assert!(has_meta("/a\\b", PathStyle::Unix));
    assert!(!has_meta("C:/a/b", PathStyle::Windows));
    assert!(!has_meta("/plain/path.txt", PathStyle::Unix));
}

// ── globbing against a real folder ──────────────────────────────────────────

#[test]
fn glob_expands_like_gos_filepath_glob() {
    let root = scratch("glob");
    for f in ["a/x.txt", "a/y.log", "b/c/z.txt", "b/w.txt", ".hid/h.txt", "top.txt", "sp ace/s.txt", "n1.dat", "n2.dat"] {
        touch(&root.join(f));
    }
    let r = canon(root.to_str().unwrap());
    let g = |pat: &str| -> Vec<String> {
        glob(&format!("{r}/{pat}"), style(), 100)
            .unwrap()
            .into_iter()
            .map(|p| p.strip_prefix(&format!("{r}/")).unwrap().to_string())
            .collect()
    };
    // A star matches dotfiles too, and results come back sorted.
    assert_eq!(g("*.txt"), ["top.txt"]);
    assert_eq!(g("*"), [".hid", "a", "b", "n1.dat", "n2.dat", "sp ace", "top.txt"]);
    assert_eq!(g("n?.dat"), ["n1.dat", "n2.dat"]);
    assert_eq!(g("n[12].dat"), ["n1.dat", "n2.dat"]);
    assert_eq!(g("n[^1].dat"), ["n2.dat"]);
    // A glob in a directory position.
    assert_eq!(g("*/x.txt"), ["a/x.txt"]);
    assert_eq!(g("*/*.txt"), [".hid/h.txt", "a/x.txt", "b/w.txt", "sp ace/s.txt"]);
    // `**` is only a star: one component, not any depth.
    assert_eq!(g("**/z.txt"), Vec::<String>::new());
    assert_eq!(g("**/w.txt"), ["b/w.txt"]);
    assert_eq!(g("b/**/z.txt"), ["b/c/z.txt"]);
    // A plain path matches if it exists; nothing otherwise.
    assert_eq!(g("a/x.txt"), ["a/x.txt"]);
    assert_eq!(g("a/missing.txt"), Vec::<String>::new());
    assert_eq!(g("nomatch*"), Vec::<String>::new());
    // A pattern whose directory part isn't a directory yields nothing, silently.
    assert_eq!(g("top.txt/*"), Vec::<String>::new());
    // Malformed.
    assert!(glob(&format!("{r}/[a"), style(), 10).is_err());
    // The result limit.
    assert_eq!(glob(&format!("{r}/*"), style(), 3).unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(&root);
}

// ── reading list files ──────────────────────────────────────────────────────

#[test]
fn entries_carry_their_file_line_and_pattern() {
    let root = scratch("entries");
    for f in ["a/x.txt", "a/y.log", "top.txt", "t2.txt"] {
        touch(&root.join(f));
    }
    let r = canon(root.to_str().unwrap());
    let text = format!("# mine\n\n{r}/a\n  {r}/*.txt  \n{r}/missing\n{r}/[bad\nrelative/dir\n{r}/a\n");
    let list = [("/lists/one.txt", text.as_str())];
    let got = read_list_files(&["/lists/one.txt".to_string()], style(), &files(&list));

    assert_eq!(paths(&got), [format!("{r}/a"), format!("{r}/t2.txt"), format!("{r}/top.txt")]);
    assert_eq!(got.entries[0].line, 3);
    assert_eq!(got.entries[0].pattern, format!("{r}/a"));
    assert_eq!((got.entries[1].line, got.entries[1].file.as_str()), (4, "/lists/one.txt"));
    assert_eq!(got.entries[1].pattern, format!("{r}/*.txt"));
    assert!(!got.truncated);

    // The glob that matches nothing, the malformed pattern and the relative entry are reported by
    // line; the repeat of line 3 is silently the same source.
    let whats: Vec<&str> = got.problems.iter().map(|p| p.what.as_str()).collect();
    assert_eq!(whats, ["/lists/one.txt:5", "/lists/one.txt:6", "/lists/one.txt:7"]);
    assert!(got.problems[0].error.contains("matches nothing"));
    assert!(got.problems[1].error.contains("restic won't run"));
    assert!(got.problems[2].error.contains("relative path"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_file_that_cant_be_read_is_a_problem_not_a_failure() {
    let got = read_list_files(&["/nope.txt".to_string(), "  ".to_string()], style(), &files(&[]));
    assert!(got.entries.is_empty());
    assert_eq!(got.problems.len(), 1);
    assert_eq!(got.problems[0].what, "/nope.txt");
    assert!(got.problems[0].error.contains("can't read path-list file"));
}

#[test]
fn many_files_keep_their_order_and_a_shared_entry_belongs_to_the_first() {
    let root = scratch("order");
    for f in ["a", "b", "c"] {
        touch(&root.join(f));
    }
    let r = canon(root.to_str().unwrap());
    let one = format!("{r}/b\n{r}/a\n");
    let two = format!("{r}/*\n");
    let list = [("one", one.as_str()), ("two", two.as_str())];
    let got = read_list_files(&["one".to_string(), "two".to_string()], style(), &files(&list));
    assert_eq!(paths(&got), [format!("{r}/b"), format!("{r}/a"), format!("{r}/c")]);
    assert_eq!(got.entries[0].file, "one");
    assert_eq!(got.entries[2].file, "two");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_entry_cap_stops_the_reading_and_says_so() {
    let root = scratch("cap");
    for i in 0..12 {
        touch(&root.join(format!("f{i:02}")));
    }
    let r = canon(root.to_str().unwrap());
    let text = format!("{r}/*\n");
    let list = [("l", text.as_str())];
    let got = read_with_limit(&["l".to_string()], style(), &files(&list), 5);
    assert_eq!(got.entries.len(), 5);
    assert!(got.truncated);
    assert!(got.problems.iter().any(|p| p.error.contains("only the first 5")));
    let _ = std::fs::remove_dir_all(&root);
}

// ── the engine treats list entries as sources ───────────────────────────────

#[test]
fn list_sources_are_sources_to_the_engine_but_never_the_edits_to_rewrite() {
    use crate::commands::plan_edit::{apply_edit, EditAction};
    use crate::commands::plan_rules::FsProbe;
    use crate::commands::plan_tree::TreeRulesInput;

    let root = scratch("engine");
    for f in ["docs/a.txt", "docs/node_modules/m.js", "other/o.txt"] {
        touch(&root.join(f));
    }
    let r = canon(root.to_str().unwrap());
    let input = TreeRulesInput {
        list_sources: vec![format!("{r}/docs")],
        excludes: vec!["node_modules".into()],
        style: style(),
        ..Default::default()
    };
    let probe = FsProbe::default();
    let read = |p: &str| std::fs::read_to_string(p);
    let getenv = |_: &str| String::new();
    let built = crate::commands::plan_tree::build_rules(&input, &read, &getenv);
    let engine = crate::commands::plan_rules::Engine::new(&built.rules).unwrap();
    assert!(engine.classify(&format!("{r}/docs/a.txt"), &probe).included);
    assert!(!engine.classify(&format!("{r}/docs/node_modules"), &probe).included);
    assert!(!engine.classify(&format!("{r}/other/o.txt"), &probe).included);
    assert!(engine.classify(&r, &probe).reason.is_some()); // holds a source, not backed up itself

    // Including something outside adds a plan source; the list source stays out of the outcome.
    let include = EditAction { kind: "include".into(), path: format!("{r}/other"), pattern: None, scope: None };
    let out = apply_edit(&input, &include, &probe, &read, &getenv).unwrap();
    assert_eq!(out.sources, [PathStyle::NATIVE.to_native(&format!("{r}/other"))]);
    assert!(out.removed_sources.is_empty());

    // Excluding the list source by its path can't be done from here, and the message says why.
    let exclude = EditAction { kind: "exclude".into(), path: format!("{r}/docs"), pattern: Some(format!("{r}/docs")), scope: None };
    let err = apply_edit(&input, &exclude, &probe, &read, &getenv).unwrap_err();
    assert!(err.contains("path-list file"), "{err}");

    // But excluding something inside it works like anywhere else.
    let inside = EditAction { kind: "exclude".into(), path: format!("{r}/docs/a.txt"), pattern: Some(format!("{r}/docs/a.txt")), scope: None };
    let out = apply_edit(&input, &inside, &probe, &read, &getenv).unwrap();
    assert!(out.sources.is_empty());
    assert_eq!(out.added_rules.len(), 1);
    let _ = std::fs::remove_dir_all(&root);
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

const NAMES: &[&str] = &["a", "b", "c", "src", "deep", "node_modules", ".env", "x.log", "keep.txt", "we[ird]", "sp ace", "n1", "n2"];

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

/// Random list files — explicit paths, globs, things that don't exist, comments, blanks, padding,
/// CRLF — expanded by `read_list_files` and judged by the engine, against `restic backup
/// --dry-run --files-from`, node by node. Needs `restic` on PATH:
/// `cargo test restic_reads_list_files -- --ignored --nocapture`.
#[test]
#[ignore]
fn restic_reads_list_files_like_the_engine_does() {
    use crate::commands::plan_rules::FsProbe;
    use crate::commands::plan_rules::Engine;
    use crate::commands::plan_tree::{build_rules, TreeRulesInput};
    use rand::{Rng, SeedableRng};

    if !restic_available() {
        eprintln!("restic not found on PATH; skipping");
        return;
    }
    let base = scratch("restic");
    let repo = base.join("repo");
    assert!(std::process::Command::new("restic").args(["init", "--repo"]).arg(&repo).env("RESTIC_PASSWORD", "pw").output().unwrap().status.success());
    let esc = |s: &str| PathStyle::NATIVE.escape_glob(s);

    let cases: u64 = std::env::var("RESTY_DIFF_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(80);
    let seed: u64 = std::env::var("RESTY_DIFF_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let (mut checked, mut entries_total, mut cases_run) = (0usize, 0usize, 0usize);
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
        let r = canon(root.to_str().unwrap());
        let dirs: Vec<String> =
            std::iter::once(r.clone()).chain(nodes.iter().filter(|p| p.is_dir()).map(|p| canon(p.to_str().unwrap()))).collect();

        // The list file.
        let mut lines: Vec<String> = Vec::new();
        for _ in 0..rng.gen_range(1..6) {
            let n = canon(nodes[rng.gen_range(0..nodes.len())].to_str().unwrap());
            let dir = &dirs[rng.gen_range(0..dirs.len())];
            lines.push(match rng.gen_range(0..12) {
                0..=2 => esc(&n),
                3 => format!("{}/*", esc(dir)),
                4 => format!("{}/*.txt", esc(dir)),
                5 => format!("{}/n?", esc(dir)),
                6 => format!("{}/[ab]*", esc(dir)),
                7 => format!("{}/*/src", esc(dir)),
                8 => format!("{}/**/keep.txt", esc(dir)),
                9 => format!("{}/missing-{}", esc(dir), rng.gen_range(0..9)),
                10 => format!("  {}  ", esc(&n)),
                _ => format!("# {}", esc(&n)),
            });
            if rng.gen_range(0..4) == 0 {
                lines.push(String::new());
            }
        }
        let eol = if rng.gen_range(0..3) == 0 { "\r\n" } else { "\n" };
        let text = lines.join(eol) + eol;
        let list_file = base.join(format!("list{case}.txt"));
        std::fs::write(&list_file, &text).unwrap();
        let list_path = canon(list_file.to_str().unwrap());

        // A few exclude patterns, so the entries' own exemption is exercised too.
        let mut excludes: Vec<String> = Vec::new();
        for _ in 0..rng.gen_range(0..3) {
            let n = canon(nodes[rng.gen_range(0..nodes.len())].to_str().unwrap());
            let name = n.rsplit('/').next().unwrap().to_string();
            excludes.push(match rng.gen_range(0..3) {
                0 => esc(&name),
                1 => format!("{}/*", esc(&n)),
                _ => "*.txt".to_string(),
            });
        }

        let expanded = read_list_files(std::slice::from_ref(&list_path), style(), &|p| std::fs::read_to_string(p));
        entries_total += expanded.entries.len();
        let input = TreeRulesInput {
            list_sources: expanded.entries.iter().map(|e| e.path.clone()).collect(),
            excludes: excludes.clone(),
            style: style(),
            // Entries are source paths too: exempt from the excludes from restic 0.19, judged before.
            judge_sources: crate::commands::test_support::restic_judges_sources(),
            ..Default::default()
        };
        let read = |p: &str| std::fs::read_to_string(p);
        let getenv = |_: &str| String::new();
        let engine = Engine::new(&build_rules(&input, &read, &getenv).rules).unwrap();
        let probe = FsProbe::default();

        let mut cmd = std::process::Command::new("restic");
        cmd.args(["backup", "--repo"]).arg(&repo).args(["--dry-run", "-vv", "--json", "--no-scan", "--files-from"]).arg(&list_file);
        for e in &excludes {
            cmd.args(["--exclude", e]);
        }
        let out = cmd.current_dir(&base).env("RESTIC_PASSWORD", "pw").output().expect("run restic");
        let reported: HashSet<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|j| j["message_type"] == "verbose_status")
            .filter_map(|j| j["item"].as_str().map(|i| crate::commands::test_support::restic_item_path(i, cfg!(windows)).trim_end_matches('/').to_string()))
            .collect();

        let entry_paths: Vec<&str> = expanded.entries.iter().map(|e| e.path.as_str()).collect();
        for node in nodes.iter().filter(|p| p.exists()) {
            let p = canon(node.to_str().unwrap());
            // restic also reports the folders on the way down to an entry, which aren't sources.
            if entry_paths.iter().any(|e| e.starts_with(&format!("{p}/"))) {
                continue;
            }
            assert_eq!(
                engine.classify(&p, &probe).included,
                reported.contains(&p),
                "case {case} (seed {seed}): engine vs restic on {p}\nlist file ({} bytes):\n{text}\nexcludes {excludes:?}\nentries {entry_paths:?}",
                text.len()
            );
            checked += 1;
        }
        cases_run += 1;
    }
    eprintln!("restic agrees with the list-file reader on {checked} nodes over {cases_run} cases ({entries_total} entries)");
    assert!(checked > 0);
    let _ = std::fs::remove_dir_all(&base);
}
