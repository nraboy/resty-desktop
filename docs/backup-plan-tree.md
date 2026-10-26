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

## Windows and Unix path rules (`PathStyle`)

The matcher works on path *components*, so the only platform-specific parts are isolated in
`PathStyle` (`Unix` | `Windows`, `PathStyle::NATIVE` for the build target); `PlanRules.style`
selects it, and the tree/edit commands take the app's native style. On Windows:

- `\` and `/` are both separators and are normalised to `/`; a path may start with a drive
  (`C:`). Internally every path is **canonical** (`C:/Users/me`, drive roots `C:/`); `to_native`
  gives the OS spelling back, and `plan_edit`'s `finish` converts every source it hands to the
  frontend (and every exported one) to the native spelling, so the plan keeps `C:\Users\me`.
- There is no backslash escape in a pattern (it is a separator), so a literal `[` is written as the
  class `[[]` (`escape_glob`). The frontend mirrors this (`escapeGlob(s, windows)`).
- Drive-letter patterns match like restic's "relative-style" patterns (a window of components, not
  anchored to the root); a pattern such as `C:/` hits restic's empty-part-as-`**` quirk, which is
  mirrored. **UNC paths (`\\server\share`) are not supported.**
- CLI export (`shell_quote`) targets `sh` on Unix and **PowerShell** on Windows, and uses single
  quotes in both (fully literal: no `$(…)`, `$var`, backtick or `%` expansion — double quotes would
  let a folder named `$(calc)`, e.g. from an imported plan, run when the command is pasted). Only a
  conservative set of plain characters stays unquoted (not `,`, `@`, `%`, `=` on Windows), and
  PowerShell's typographic single quotes (‘ ’ ‚ ‛) are doubled like the ASCII one — a folder called
  `it’s` would otherwise end the literal early. An
  argument with a control character (or a Unicode line/paragraph separator), or a `"` on Windows,
  can't be quoted safely: the command is
  then omitted with a note and only the exclude file(s) are offered. cmd.exe users are told to swap
  the quotes.
- Case: matching stays case-sensitive (restic's behaviour); only `--iexclude*` rules fold case.

The Windows rules are pure string logic, so they are unit-tested on every OS, including restic's
own pattern tables re-run under Windows rules (`restic_pattern_tables_hold_under_windows_rules_too`).
What a macOS/Linux machine cannot prove is the end-to-end behaviour on a real Windows filesystem
(drive enumeration, hidden attribute, real restic dry runs): that is what
`.github/workflows/restic-compat.yml` is for.

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
  The ordinary test job has no restic, so these are skipped there; `.github/workflows/restic-compat.yml`
  installs restic 0.17.3 (the supported minimum) and 0.19.1 on Linux, macOS and Windows and runs
  every `plan_` and `restic_` test with `--include-ignored` (on PRs/pushes touching `plan_*` files,
  or on demand via "Run workflow"). Still worth running locally after touching the engine.
  The harnesses are platform-aware (`PathStyle::NATIVE`), so on Windows they compare the Windows
  rules against real Windows restic.

## Performance (single thread, release build, 13 rules)

`judge_node` ≈ 1.2 µs per node (≈ 850k nodes/s); `classify` ≈ 6 µs (it re-judges every ancestor —
the tree should use `judge_node` incrementally). `cargo test engine_speed --release -- --ignored
--nocapture` reproduces it. Targets for the tree UI built on top (not yet measured): first paint
< 100 ms, expanding a 10k-entry folder < 200 ms, virtualised rows, scans that never enter excluded
folders, a per-folder time budget with a "not fully checked" state.

## The tree preview (phase 2, read-only)

`components/PlanTreePreview.tsx` is a "Preview" card in the plan editor (open by default, collapsible) (after the
exclude sections). It is **additive**: the existing source/exclude editors are unchanged, and the
card shows a message for plans that use path-list files. `tree_supported` is now true on every
platform (kept as the single switch should a platform need turning off).

- **Roots:** *Sources* (each source not nested in another; nested ones appear inside their parent)
  or *Whole filesystem* (`/` on Unix; on Windows one root per drive, from `tree_env`, auto-expanded
  down to each source). Rows are marked **included**
  (filled box), **partly included** (dash: backed up, but something inside is excluded),
  **excluded** (empty box, greyed), plus *contains backed-up items* for a folder that sits above a
  source (shown as partly), and *not fully checked* for a huge included folder the scan couldn't
  finish within its budget. Hover a name for the reason in plain language ("Excluded because its
  folder X is excluded by `node_modules`", "…by line 12 of ~/.ignore", "Re-included by `!…`").
- **`tree_env`** reports the platform (`windows`), the roots (`/`, or the logical drives via
  `GetLogicalDrives`) and the home folder; entries carry a `hidden` flag (dot-name on Unix/macOS,
  the hidden attribute on Windows) for the planned "show hidden files" toggle.
- **Backend (`commands/plan_tree.rs`):** `tree_list_dir` (names + entry types only, folders first,
  capped at 3000 with a "…and N more" note), `tree_kinds` (is a source a folder, file or gone),
  `tree_classify` (verdicts for a batch of paths, plus each rule's origin: plan pattern, or file
  and line), `tree_scan_start`/`tree_scan_cancel`. `build_rules` assembles the engine's input
  exactly as restic receives it — `--exclude` flags, then exclude-file lines (trimmed, `#` and blanks
  skipped, environment variables expanded like Go's `os.Expand`), markers, `--exclude-caches` — and
  reports invalid patterns and unreadable files instead of failing.
- **Scans** answer "is anything inside excluded?" for an included folder: breadth-first (shallow
  `node_modules`/`.git` found fast), stopping at the first exclusion, never entering a rejected
  directory or a symlink, with a time and entry budget (then *not fully checked*). Up to three
  worker threads; starting a scan supersedes the previous one, so the UI re-sends whatever is still
  unresolved. Results stream back as `tree:scan` events (`tree:scan-done` at the end), folded into
  the view every 100 ms. With no rules at all nothing is read.
- **Nothing persists:** listings, verdicts and scan results live in the component's refs until the
  editor closes; there is no table and nothing for Clear Cache to clean. Rule edits are debounced
  (350 ms), invalidate verdicts and scans (listings are kept), and re-check the visible rows first.
- **Rendering:** fixed-height rows, windowed (only the viewport plus overscan is in the DOM — a
  1,500-entry folder renders ~25 rows), data in refs with a counter to re-render. Checked in a
  browser harness against a mocked backend (layout, virtualisation, live rule edits, tooltips);
  the Rust commands are covered by unit tests, but the event path between them is only exercised in
  the real app.

## Editing from the tree (phase 3, slice 1)

Clicking a row's box opens a dialog (`PlanTreeEditDialog`); nothing changes until the user confirms,
and the Preview card shows "Updated your plan: … Undo" afterwards. The existing Paths/Exclude
editors stay in place and simply show the result (the old Simple list is the rules list; Expert
text keeps its comments). Include patterns (the `.env`-anywhere idea) are deliberately deferred.

- **Exclude** offers pattern scopes for the clicked path — this exact item, a folder's contents
  only, the name anywhere, the name under a nearer ancestor (never above the source), a file
  extension — as an editable pattern, with a live "would leave out N items — e.g. …" from a
  bounded scan (`tree_count_matches`: items included now that the new pattern would reject, a
  folder counted once). Paths and names are glob-escaped (`escape_glob`), so a folder called
  `we[ird]` is excluded literally.
- **Include** is computed up front (`tree_plan_edit`) and the exact sources/patterns it will add are
  shown: an item outside every source becomes a source (absorbing sources nested in it); a folder
  excluded by a pattern gets `!folder`; an item below an excluded folder gets the carve-out chain
  `!X`, `X/*`, `!X/child`, …, `!target` (or `!X` for "the whole folder"). Excluding a source by its
  exact path removes the source.
- **Every edit is verified by the engine** (`plan_edit.rs::apply_edit`) after being applied to a
  copy of the plan: the clicked item must really end up as asked, otherwise the click is refused
  with a reason — an ancestor excluded by a marker file, a pattern in an exclude file (applied after
  the plan's own patterns, so a `!` can't override it; the message names file and line), a
  case-insensitive rule (a separate list `!` can't cancel), or a source root that patterns don't
  apply to. New rules are appended last so they take precedence over the plan's earlier ones.
- **Verification against restic:** `restic_confirms_every_edit_the_engine_offers` (`#[ignore]`,
  needs restic) applies random clicks to random plans and trees and checks with real dry runs that
  the clicked item ended up as asked and the engine agrees on every node — last run 360 edits
  (~240 re-includes, many from deep inside excluded folders), ~5,800 nodes, zero disagreements.
  Run it after touching plan_edit.rs: `cargo test restic_confirms -- --ignored --nocapture`.
- **CLI parity (`tree_export`, "Use with restic CLI…"):** everything the GUI generates is a plain
  restic pattern, so the plan's rules render as the files and command a user would run by hand:
  one ordered `restic-excludes.txt` (the flag patterns and every exclude file's lines, in the order
  restic evaluates them — one list, so `!` works across all of it), `restic-iexcludes.txt` for
  case-insensitive rules (restic keeps those in a separate list), `--exclude-if-present` /
  `--exclude-caches` as flags, and the sources. In files restic expands `$VAR` on every line, so a
  literal `$` is written `$$`. The repository and password are never written (they come from
  `RESTIC_REPOSITORY`/`RESTIC_PASSWORD`), and non-inclusion settings (tags, bandwidth, compression)
  aren't in the command. `restic_confirms_exported_files_match_the_gui_flags` (`#[ignore]`) checks
  that `--exclude-file` gives the same included set as the GUI's `--exclude` flags (1,477 reported
  nodes over 80 random cases). The one feature that would break parity by construction is the
  deferred include-pattern resolution (a runtime-built file list); don't add it without an export
  story.

## Status / roadmap

1. **Engine + tests (`plan_rules.rs`)** — done, including Windows path semantics (`PathStyle`);
   the Windows end-to-end run is delegated to the `restic-compat` workflow (not yet seen green).
2. **Read-only tree** — done (this section); keyboard navigation (arrow keys) and a filter box for
   huge folders are not yet.
3. Editing — slice 1 done (above): click to include/exclude with the pattern dialog, verified edits,
   undo, CLI export. Still to do: copy-on-import of exclude files, a rules list that replaces the
   Paths/Exclude editors, and (deferred) include patterns resolved to an explicit
   `--files-from-verbatim` list at backup time (restic has no include patterns).
4. Polish: size estimates, keyboard/accessibility; verifying on a real Windows machine.
