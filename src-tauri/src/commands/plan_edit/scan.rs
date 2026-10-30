//! Bounded walks over the files a plan covers, for the two questions a tree edit has to answer
//! about more than the clicked item: did the edit change anything it shouldn't have
//! ([`find_collateral`]), and what did re-including a folder with `!` bring back that other rules
//! exclude ([`count_brought_back`]).
//!
//! Both walk breadth first and never enter an item restic would not enter (a rejected folder, a
//! symlink), the way `plan_tree::count_matches` does, and give up at the same kind of budget. They
//! run on every click, so [`COLLATERAL_LIMITS`] is tighter than the live match count's: a scan that
//! runs out of budget simply doesn't deliver a verdict, and the edit goes ahead on what the engine
//! has already confirmed.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::commands::plan_rules::{strip_source, Engine, Probe};
use crate::commands::plan_tree::{MatchPreview, ScanLimits};

/// What one click may spend on the scans above.
pub(crate) const COLLATERAL_LIMITS: ScanLimits = ScanLimits { budget: Duration::from_millis(750), max_entries: 2_000_000 };

/// A re-included folder rarely hides more than this many things; beyond it the count is "at
/// least" (`count_matches` stops at the same number).
const BROUGHT_BACK_CAP: u64 = 5000;
const BROUGHT_BACK_SAMPLE: usize = 5;

/// The first item below a source whose verdict differs between `before` and `after` and which
/// is not `target`, inside it, or on the way down to it — those are what a click on `target` is
/// allowed to change. Walks the sources of `after` breadth first, entering a folder when either
/// engine still reaches it (a folder both leave out has nothing to compare), and never enters
/// `target` (all of it is the click's business). `None` when nothing else changed, or when the
/// scan ran out of budget before it could say.
pub(crate) fn find_collateral(
    before: &Engine,
    after: &Engine,
    probe: &dyn Probe,
    target: &str,
    limits: ScanLimits,
) -> Option<String> {
    if !before.has_rules() && !after.has_rules() {
        return None; // everything under a source is included either way
    }
    let style = after.style();
    let target = style.clean(target);
    let in_target = |p: &str| strip_source(p, &target).is_some();
    let on_the_way = |p: &str| strip_source(&target, p).is_some();

    let sources = after.sources();
    let mut queue: VecDeque<String> = VecDeque::new();
    for src in sources {
        // A source inside another is walked as part of it (the engine reaches it from there).
        if sources.iter().any(|o| strip_source(src, o).is_some_and(|rel| !rel.is_empty())) {
            continue;
        }
        let (b, a) = (before.classify(src, probe).included, after.classify(src, probe).included);
        if b != a && !in_target(src) && !on_the_way(src) {
            return Some(src.clone());
        }
        if !in_target(src) && (b || a) {
            queue.push_back(src.clone());
        }
    }

    let start = Instant::now();
    let mut scanned: u64 = 0;
    while let Some(current) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(style.to_native(&current)) else { continue };
        for ent in rd.flatten() {
            scanned += 1;
            if scanned > limits.max_entries || (scanned & 0xFF == 0 && start.elapsed() > limits.budget) {
                return None;
            }
            let Some(name) = ent.file_name().to_str().map(str::to_string) else { continue };
            let path = style.join(&current, &name);
            let (b, a) = (before.classify(&path, probe).included, after.classify(&path, probe).included);
            if in_target(&path) {
                continue; // the click's own: allowed to change, nothing below it to look at
            }
            if b != a && !on_the_way(&path) {
                return Some(path);
            }
            // `file_type` doesn't follow links: a symlink to a folder is stored as a link.
            if (b || a) && ent.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push_back(path);
            }
        }
    }
    None
}

/// How many items inside `folder` `others` would leave out: a `!` on `folder` re-includes those
/// too (restic lets a `!` win over every earlier rule about anything below it). A folder `others`
/// rejects counts once and is not entered. The count stops at [`BROUGHT_BACK_CAP`] or `limits`
/// (`capped`: there are at least that many).
pub(crate) fn count_brought_back(others: &Engine, probe: &dyn Probe, folder: &str, limits: ScanLimits) -> MatchPreview {
    let mut out = MatchPreview { count: 0, sample: vec![], capped: false };
    if !others.has_rules() {
        return out;
    }
    let style = others.style();
    let start = Instant::now();
    let mut scanned: u64 = 0;
    let mut queue: VecDeque<String> = VecDeque::from([style.clean(folder)]);
    while let Some(current) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(style.to_native(&current)) else { continue };
        for ent in rd.flatten() {
            scanned += 1;
            if scanned > limits.max_entries || (scanned & 0xFF == 0 && start.elapsed() > limits.budget) {
                out.capped = true;
                return out;
            }
            let Some(name) = ent.file_name().to_str().map(str::to_string) else { continue };
            let path = style.join(&current, &name);
            if others.judge_node(&path, probe).is_some() {
                out.count += 1;
                if out.sample.len() < BROUGHT_BACK_SAMPLE {
                    out.sample.push(path);
                }
                if out.count >= BROUGHT_BACK_CAP {
                    out.capped = true;
                    return out;
                }
                continue; // left out with everything below it
            }
            if ent.file_type().is_ok_and(|t| t.is_dir()) {
                queue.push_back(path);
            }
        }
    }
    out
}
