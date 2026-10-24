# Backup-plan rule engine and file tree

Read this before: touching `commands/plan_rules.rs`, or building anything that decides what a
backup plan includes (the plan editor's file-tree view, include/exclude editing, previews).
Linked from CLAUDE.md's "Where the detail lives" table.

## Why a Rust port of restic's matching

The planned file tree shows, for every folder and file, whether `restic backup` would include it,
leave it out, or include it only partly, and which rule decided. That is only trustworthy if the
answer is **exactly restic's**, quirks included, so `plan_rules.rs` is a literal port of restic
0.17+'s `internal/filter` (`Match`, `ChildMatch`, `List`) and `internal/archiver`
(`RejectIfPresent`) on top of a port of Go's `path/filepath.Match` — not a reinterpretation.
Don't "simplify" it: every oddity below was found by running restic.

## Semantics the engine reproduces

(All verified against restic 0.19.1 dry runs: `restic backup --dry-run -vv --json`.)

- **A pattern matches any contiguous run of path components**, not only the tail; only a leading
  `/` anchors it. `node_modules` excludes that name at any depth; an absolute pattern also matches
  everything beneath it (prefix match). `**` consumes any number of components.
- **Evaluation is ordered and `!` cancels an earlier match.** A `!` listed before the exclude it
  should cancel does nothing.
- **List order reaching restic:** all `--exclude` flags (in order), then the contents of
  `--exclude-file` files appended. `--iexclude`/`--iexclude-file` form a *separate* list
  (lower-cased matching); a case-sensitive `!` cannot cancel a case-insensitive exclude and vice
  versa. A node is rejected if either list rejects it.
- **restic judges every node it visits and never descends into a rejected directory.** So a node
  is included only if it and *every ancestor below its source root* pass
  (`Engine::classify`). Re-including a file inside an excluded directory is impossible; the
  workaround is excluding the directory's *contents* (`dir/*`) and re-including the child
  (`!dir/child`), or the chain `!X`, `X/*`, `!X/child`.
- **A source path is exempt itself, not its contents.** An explicit file source inside an excluded
  directory is backed up; an explicit directory source is, but its contents are still judged (a
  prefix rule still excludes them).
- **`exclude-if-present name[:header]`:** every item whose *parent directory* holds the marker file
  (header bytes must match, if given) is rejected, except the marker itself; the directory stays.
  `--exclude-caches` is `CACHEDIR.TAG:Signature: 8a477f597d28d172789f06886806bc55`.
- **A restic quirk reproduced on purpose:** its `**` expansion stops at `len(path) - len(current
  pattern) + 1` components, counting later `**` as parts, so patterns with several `**` can fail
  to match paths a looser reading would accept (`match_wild`'s `cur_len`).
- File-based patterns have environment variables expanded and `#`/blank lines skipped; `--exclude`
  flag patterns are used verbatim. (The file reader is not part of this module yet.)

## API

- `Engine::new(&PlanRules)` compiles sources + ordered `RuleInput`s (`id`, `pattern`, `ignore_case`) +
  markers; a malformed pattern is an error (restic refuses to run).
- `Engine::judge_node(path, &probe)` — would restic reject *this node by itself* (pattern or
  marker)? The cheap, incremental call a tree uses once it knows the parent is included.
- `Engine::classify(path, &probe)` — hierarchical verdict across all sources, with the `Reason`
  (`Pattern{rule_id}` / `Marker{spec}` / `OutsideSources`), the rejected ancestor (`at`) and the `!`
  rule that re-included it (`reincluded_by`), for "why is this excluded?" tooltips.
- `PatternList::evaluate` also reports `child_may_match` (restic's `ChildMatch`): the primitive for
  rule analysis — "could anything under this folder be rejected?" — with no disk access.
- `Probe` abstracts marker lookups (`FsProbe` = real disk with a per-directory cache).

## Testing

- **Table tests lifted from the originals:** Go's `match_test.go` and restic's `filter_test.go`
  (`plan_rules/test_tables.rs`, generated — don't edit by hand).
- **Index equivalence:** the speed index (literal names in a hash map) is checked against trying
  every pattern, including `child_may_match`, on random data. The allocation-free `**` matcher is
  checked against the literal port (`match_parts_reference`) on 20,000 random patterns.
- **Differential test against the real restic** (`restic_agrees_with_the_engine_on_random_trees_and_rules`,
  `#[ignore]`d because it needs `restic` on PATH): random trees, random rule sets (negations,
  `**`, anchored, case-insensitive, markers), engine vs. a dry run, node by node. Run:
  `cargo test restic_agrees -- --ignored --nocapture` (`RESTY_DIFF_SEED`, `RESTY_DIFF_CASES`).
  Last run: ~11,000 nodes over ~900 random cases across several seeds, zero disagreements.
  CI does not install restic yet, so this is a local/manual gate — run it after touching the engine.

## Performance (single thread, release build, 13 rules)

`judge_node` ≈ 1.2 µs per node (≈ 850k nodes/s); `classify` ≈ 6 µs (it re-judges every ancestor —
the tree should use `judge_node` incrementally). `cargo test engine_speed --release -- --ignored
--nocapture` reproduces it. Targets for the tree UI built on top (not yet measured): first paint
< 100 ms, expanding a 10k-entry folder < 200 ms, virtualised rows, scans that never enter excluded
folders, a per-folder time budget with a "not fully checked" state.

## Status / roadmap

1. **Engine + tests (this file's module)** — done. Windows path semantics (`C:` components, `\`
   separators) are not handled yet.
2. Read-only tree: whole-filesystem browser, three states, rule provenance, streaming scans.
3. Editing: SourceTree-style include/exclude dialog (exact path / name anywhere / scoped pattern),
   rules list, copy-on-import of exclude files, include patterns resolved to an explicit
   `--files-from-verbatim` list at backup time (restic has no include patterns).
4. Polish: size estimates, Windows, keyboard/accessibility.
