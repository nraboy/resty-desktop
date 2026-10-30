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
- **A source path is exempt itself, not its contents — from restic 0.19.** (restic#5767, 0.19.0
  "Change": paths named on the command line are exempt from excludes, markers and caches.) An
  explicit file source inside an excluded directory is backed up; an explicit directory source is,
  but its contents are still judged (a prefix rule still excludes them). **Before 0.19 a source is
  judged like any other node**: a pattern or marker that matches it drops it, and nothing below it
  is reached. The engine does both: `PlanRules::judge_sources` selects the old rule. The default is
  the 0.19 behaviour; `tree_env` reads `restic version` into `TreeEnv.restic_judges_sources`, the
  tree sends it back as `TreeRulesInput.judge_sources`, and the editor says so in a one-line note
  when it applies. An unreadable version means 0.19 behaviour. `count_matches` (the pattern
  dialog's live count) also counts a source a new pattern would drop under the old rule.
- **Nested explicit sources: the folders of the target tree are kept, not judged.** With sources
  `/s` and `/s/a/b`, `/s` and `/s/a` are folders of restic's target tree: a marker or pattern that
  would reject `/s/a` does not stop restic reaching `/s/a/b`, `/s/a` itself is in the snapshot, and
  the other children (`/s/a/other`) are judged one by one (`Engine::leads_to_a_source`). So even
  before 0.19 only a source with **no other source below it** is judged (`Engine::judges_source`).
  Found by the random multi-source cases; the same on 0.17.3 and 0.19.1. (A harness gotcha: a
  runner's temp path can match a loose pattern such as `n*` — macOS puts `n6` in it — which rejects
  *everything* below it; engine and restic still agree, it just makes those cases uninformative.)
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
  rules against real Windows restic. Two things make that work: restic on Windows reports an item
  as its *snapshot path* (`/C/Users/me/x`: the drive becomes the first folder), which
  `test_support::restic_item_path` maps back to `C:/Users/me/x`; and the tests that use real temp
  directories build their rules from canonical spellings (no `\\?\` prefix, `/` separators).
  Each matrix entry compares against **its own restic's** source rule: the harnesses call
  `test_support::restic_judges_sources()` (parses `restic version`) and half of the random cases
  name one or two extra explicit sources inside the tree, which is where 0.17 and 0.19 differ.

## Performance (single thread, release build, 13 rules)

`judge_node` ≈ 1.2 µs per node (≈ 850k nodes/s); `classify` ≈ 6 µs (it re-judges every ancestor —
the tree should use `judge_node` incrementally). `cargo test engine_speed --release -- --ignored
--nocapture` reproduces it. Targets for the tree UI built on top (not yet measured): first paint
< 100 ms, expanding a 10k-entry folder < 200 ms, virtualised rows, scans that never enter excluded
folders, a per-folder time budget with a "not fully checked" state.

## The tree in the Files tab (phase 2 preview, now also the source editor)

`components/PlanTreePreview.tsx` is the first card of the plan editor's **Files** tab (the plan form,
tabs and undo history live in `components/plan/` and `lib/planForm.ts`; see docs/frontend.md). The
sources of a plan are **the folders and files picked in this tree** — `SourcesCard` (a plain path
list) only renders when `tree_supported` is false, which no platform does today. It is shown for
every plan; the entries of its path-list files appear read-only (see below). The tree starts at the whole filesystem
(`/`, or the drives on Windows), expands down to each source, and the plan with no source starts
with nothing selected; a "Sources only" view is a shortcut for deep sources. Save needs at least one
source (a folder/file or a path-list file).

- **Clicking a row.** A left-click acts on exactly that item and never asks for a scope (`leftClick`,
  pure, tested): a **source** is removed; an item **outside every source** becomes one
  (`tree_plan_edit` computes and verifies it first); inside a source, an **included** item is excluded
  and an **excluded** one included. The one click that still opens a dialog is adding a source whose
  edit would **swallow existing sources**, **add rules** (restic < 0.19) or **delete a rule** (its own
  exact line; `needsConfirmation`): the dialog lists exactly what is added, replaced and removed and is
  the only confirmation, so a click never changes rules unseen. A right-click on a row — or the
  ContextMenu key / Shift+F10 on its box button, which opens the same menu under the button — opens the
  shared `ContextMenu` with `menuEntries` (pure, tested). The first entry is always the left-click's
  action, so a right-click is never dead: an included item gets **Exclude** and **Exclude by
  pattern…** (the pattern dialog, scopes unchanged), plus **Include everything inside it** (`!P`) on a
  partly included folder; an excluded item gets **Include**, plus **Include, plus everything inside it
  that other rules exclude** on a folder that has an exact rule (the plain click would only delete that
  line; this keeps it and appends `!P`); a source or an outside row gets its single add/remove action.
  There is deliberately no entry on a file inside an excluded folder that includes the folder too — if
  that were the intent the user would click the folder. A hint under the tree says "Click to
  include or exclude · Right-click for more options". Clicks and the menu are ignored while the
  verdicts on screen lag the last edit (`settled`) or an edit is in flight, and a row whose subtree scan
  has not come back (`pending`) has no box button and no menu.
- **Undo** is a stack (20 deep) kept by `FilesTab`: each record holds the sources/patterns before and
  after. The line under the toolbar undoes the latest and says how many earlier ones remain. Any
  change to the sources or patterns made another way (typing a pattern, the fallback list) empties
  the stack, because restoring an older state would throw that change away. The line summarises the
  edit (`summarizeEdit`: "added pattern", "removed rule", and — for a `!` on a folder — "also brought
  back N items other rules exclude", "at least N" when the scan was capped, left out when 0).
- **Source pill and states.** A source row carries a "Source" pill. A folder that is not backed up
  itself but holds a source has its own look (dashed outline + dot, "holds a source") so it can't be
  mistaken for "partly included" (backed up, but something inside is left out).
- **Hidden files.** A "Show hidden files" switch (off by default; not persisted, so the warning is
  seen). While off, hidden entries (`TreeEntry.hidden`: dot-names on Unix/macOS, the hidden
  attribute on Windows) are left out of the rows — except sources and the folders leading to one
  (`pinnedPaths`) — and each folder ends with "N hidden items not shown — M are backed up" and a
  Show link. A "!" on the switch says hidden items in the folders opened so far *are* backed up
  (`hiddenSummary`); hiding never changes what is backed up, only what is listed.
- **Navigation.** "Home" opens every folder down to the home folder (`tree_env().home`) and scrolls
  to it (the tree is windowed, so a reveal expands, then scrolls once the row exists). The tree
  height is user-resizable (drag the bar under it, or Arrow Up/Down on it; remembered per viewer in
  `localStorage`) and starts taller than the old fixed 340 px.
- **Roots:** *Whole filesystem* — `/` on Unix, one root per drive on Windows (from `tree_env`),
  auto-expanded down to each source — or *Sources only* (each source not nested in another; nested
  ones appear inside their parent). Rows are marked **included** (filled box), **partly included**
  (dash: backed up, but something inside is excluded), **holds a source** (a folder that is not
  backed up itself but sits above a source), **excluded** (empty box, greyed), and *not fully
  checked* for a huge included folder the scan couldn't finish within its budget. Hover a name for
  the reason in plain language ("Excluded because its folder X is excluded by `node_modules`",
  "…by line 12 of ~/.ignore", "Re-included by `!…`").
- **`tree_env`** reports the platform (`windows`), the roots (`/`, or the logical drives via
  `GetLogicalDrives`) and the home folder; entries carry a `hidden` flag (dot-name on Unix/macOS,
  the hidden attribute on Windows) for the "Show hidden files" switch.
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

## Path-list files in the tree (`--files-from`)

`commands/plan_listfiles.rs` reads a plan's list files the way restic does, and each line's result
becomes one more **source** — `PlanRules::list_sources` / `TreeRulesInput.list_sources`, which the
engine treats exactly like the plan's own sources (exempt themselves, contents judged; verified:
restic gives `--files-from` entries and glob matches the same root treatment as path arguments).
`tree_list_files(files)` returns `ListEntry { path, file, line, pattern }` plus `RuleProblem`s; the
tree card loads it when the list of files changes (and on Refresh), shows the entries with a
**"From list"** pill and a tooltip line ("Listed on line 3 of …" / "Matched by `…/*` on line 4 of …"),
and merges them into the rules it sends everywhere (verdicts, scans, edits, export).

What restic does, checked against 0.19.1 and reproduced (`parse_lines`, `glob`):
- Lines are trimmed (whitespace, `\r`, a BOM on the file); blank lines and lines starting with `#` —
  also indented ones — are skipped. `~` and `$VAR` are **not** expanded.
- The rest goes through Go's `filepath.Glob` (`**` is just `*`: one path component; `*` matches
  dotfiles; results sorted). A pattern matching nothing — including a path that doesn't exist — is
  skipped with a warning, not an error; a malformed one makes restic refuse to run. The tree reports
  both per line (`file:line`).
- A **relative** entry is resolved from restic's working directory, which the editor can't know:
  reported, not shown. At most 10,000 entries are shown (`MAX_LIST_ENTRIES`), then a note.

**Read-only by design.** Edits never add, remove or rewrite list sources: `apply_edit` keeps them in
`work.list_sources` (they take part in every verification but are not in the outcome), clicking a
"From list" row says which line of which file to change instead, and an exclude pattern that can't
affect one (a root is exempt) is refused with a message naming the list file. Patterns *inside* a
list source work like anywhere. The CLI export names the files (`--files-from`) rather than
inlining the entries. Opening the file from the app isn't offered: the shell plugin is deliberately
limited to http(s) links, and widening it is a decision of its own. Editing the list from the tree
(with a warning, since restic reads the file at backup time) is the natural next step.

Tested by unit tests (parsing, glob against a real folder, caps, the engine/edit behaviour) and by
`restic_reads_list_files_like_the_engine_does` (`#[ignore]`, needs restic; also in the `restic-compat`
workflow): random trees, random list files (explicit paths, `*`/`?`/`[..]`/`**`, missing paths,
comments, padding, CRLF) plus exclude patterns, engine vs a dry run, node by node — last run
2,764 nodes over 180 cases and three seeds, zero disagreements.

## Editing from the tree

A click edits the plan at once and the card shows "Updated your plan: … Undo" afterwards. The exclude
editors stay in place and simply show the result (the Simple list is the rules list, comments included;
Expert text is the same lines). Two dialogs remain (`PlanTreeEditDialog`), both reached on purpose:
**Exclude by pattern…** from the row's menu, and the **add-source confirmation** described above. Include
patterns (the `.env`-anywhere idea) are deliberately deferred.

The engine side is `commands/plan_edit.rs` (the edits and their verification), with
`plan_edit/lines.rs` (rule-list bookkeeping: a path's exact lines, `# Tree:` comments, carve-out
scaffolding — text on the stored list only, the rule engine decides what a list *means*),
`plan_edit/scan.rs` (the bounded walks: collateral finder, brought-back counter) and `plan_edit/tests.rs`
(unit tests and the restic harnesses).

**Edit kinds** (`tree_plan_edit`, `EditAction { kind, path, pattern }`; the earlier include variant that
also reopened the excluded ancestor folder is gone):

- `excludeItem` — left-click on an included item: exactly that item.
- `include` — left-click on an excluded item (and an item outside every source, which becomes a source).
- `includeEverything` — `!P`: the item and everything inside it that other rules exclude (menu only).
- `exclude` + `pattern` — the pattern dialog's choice (this exact item, a folder's contents only, the
  name anywhere, the name under a nearer ancestor, a file extension; paths and names glob-escaped, so a
  folder called `we[ird]` is excluded literally), with a live "would leave out N items — e.g. …" from a
  bounded scan (`tree_count_matches`). Excluding a source by its exact path removes the source.

`EditOutcome` carries the plan's new `sources` and full `excludes` (comment lines included), the real
patterns `addedRules`/`removedRules` (never comments), `addedSources`/`removedSources`, and `broughtBack`.
An add-source edit can return `removedRules` with no `addedRules` (it deleted the clicked path's own exact
line), so the confirmation opens on any of them.

**What the edits rely on** (restic 0.19.1 dry runs; the differential tests are the authority). `X` is a
folder, rules are in order:

- **F1** `!P` brings back P and *everything below it*, overriding earlier excludes of items below P:
  `[*.tmp, sub, !sub]` backs up `sub/*.tmp`; `[sub/c.txt, sub, !sub]` backs up `sub/c.txt`.
- **F2** Deleting P's exact line leaves other rules about P's contents in force: `[sub/c.txt, sub]` →
  delete `sub` → `c.txt` stays excluded, siblings return.
- **F3** Carve-out chain for a file directly in excluded `X`: `[X, !X, X/*, !X/a]` backs up only `a`.
  Re-appending the *full* chain for a second file (`…, !X, X/*, !X/b`) **drops `a`**, so the chain starts
  at the highest still-excluded node (for `b` that is `X/b`, so only `!X/b` is added). Keep that property.
- **F4** Scaffolding without a leaf (`[X, !X, X/*]`) backs up an *empty folder* `X`: not back to how it
  started.
- **F5** With a broad rule also excluding P (`[*.txt, a.txt]`), deleting the exact line changes nothing.
- **F6** `[X, !X, X/*, !X/a]` then `X` appended excludes everything again; then `!X` appended brings back
  *all* of X (F1).

*Exact rule* = a non-negated, case-sensitive line of the plan's **own** pattern list (never an exclude
file's) whose trimmed text equals P's path with glob characters escaped for the platform
(`PathStyle::escape_glob`; Windows writes `[` as `[[]`). A hand-typed line with that text counts
(`exact_lines`; the frontend mirrors it as `hasExactRule`).

**Including P** (`do_include`; "already included" is refused):
1. Outside every source: P becomes a source, absorbing sources nested in it.
2. Only the plan's own case-sensitive patterns can be changed. A marker, an exclude-file line or a
   case-insensitive rule that rejects P is refused with the reason.
3. P excluded at itself: its exact line(s) are deleted, newest first, until P is included. If a broader
   rule still excludes it, the lines stay deleted (they did nothing, F5) and the specific `!P` is
   appended. A broad rule (`*.png`, `node_modules`, an exclude-file line) is **never** touched.
4. P inside an excluded folder: the minimal carve-out chain from the highest still-excluded node down —
   `!X`, `X/*`, then per level `!child`, `child/*`, ending `!P`. A file directly in X takes 3 rules, each
   extra level 2, a sibling of an earlier carve-out 1 (F3). P's own exact lines stay.

**Excluding P** (`do_exclude_item`; a source is removed instead):
1. Lone `!P` lines (a tree include, "include everything", a hand-written one) are deleted newest first
   until P is excluded, restoring the earlier state. A `!P` that is carve-out scaffolding (directly
   followed by `P/*`, or with `!P/…` below it) is never deleted by a click on P itself — that would
   strand the carve-out as dead lines. If deleting them all still leaves P included they weren't what
   included it: they are put back and step 2 applies. When the deleted line was the last carve-out
   out of a folder, the scaffolding that opened the folder for it (`!A`, `A/*` per level that
   no remaining re-include below still needs) goes too, so no empty `X/` stays backed up (F4).
2. Otherwise P's exact rule is appended. The next click on P then deletes that exact line.

**The scaffolding pair is deliberate.** `orphan_scaffolding` (lines.rs) judges each `!A`/`A/*`
pair on its own — a `!` line *above* the pair doesn't keep it in use, because the pair's `A/*` overrides
it and restic applies the last matching rule — and `remove_orphan_scaffolding` (plan_edit.rs) deletes the
pairs only if every folder they reopened is excluded again without them. The two only work together:
the new pair semantics alone would delete scaffolding that is the only thing keeping a folder's
contents out (its siblings would come back), and the gate alone, with the old "any remaining re-include
under `A/` keeps the pair in use" reading, left the scaffolding in place whenever an older `!` sat above
it, so the undo left an empty folder backed up (F4) and click twice was not the identity. Never relax one
without revisiting the other.

**Scope of deletions.** A click may delete only P's exact lines, a `!P` line, the orphaned scaffolding
above and the tree comment directly above a deleted line (below). It never deletes or rewrites any other
line: broad rules, exclude-file lines and the user's own comments are never touched (the differential
harness asserts it on every click).

**Comments.** Every edit that adds rules puts one comment line directly above them, `# Tree: excluded
<path>` or `# Tree: included <path>`, plus an optional ` — note` saying why the rules look as they do
("a broader rule excludes it; this re-includes only this item", "out of the excluded X: reopen it,
exclude what is inside, bring this back"; pattern-dialog edits read "with pattern …"). One line;
control characters in a path become `?`. Adding or removing a source on its own adds no rules and no
comment. restic never sees them: `build_exclude_args`, `build_rules` and the rule counts already skip
`#` lines. A comment is
deleted with the rules it introduced and only then: it must sit directly above the first deleted line
and start with `# Tree: excluded|included <P>` for the same P (`comment_names`, matched against the
known path, no general parser). A comment for another path or one the user wrote is never deleted. A
comment left above scaffolding that a click on a different path orphaned stays: harmless. The
"Use with restic CLI" export renders parsed rules, so it carries no comments.

**Collateral check** (`scan.rs::find_collateral`). A click may flip only P, nodes inside P and the
folders on the way down to P. After computing an edit the engine builds the plan before and after and
walks the sources breadth first (750 ms / 2M entries; never into a rejected folder or a symlink),
comparing every node's verdict; if anything else flipped the edit is refused ("This change would also
change …, so it wasn't made.") and nothing is applied. A scan that runs out of budget lets the edit
through, verified on what it saw. It applies to every kind except the `exclude` pattern kind (a pattern
exists to match many items) and edits that add or remove sources.

**Brought-back count** (`scan.rs::count_brought_back`). By F1 a `!` on a *folder* also brings back items
inside it that other rules exclude. That is accepted, and disclosed: when the last added rule is the `!`
on a folder, the plan without the lines that matched the folder itself is walked (cap 5000, same budget)
and `broughtBack { count, capped }` goes into the Undo line. An item counts only when that plan leaves
it out **and** the plan after the edit includes it: what an exclude-file line (applied after the plan's
own patterns), a case-insensitive rule or a marker file still hides is not brought back by the `!`, so
it isn't counted, even when a plan pattern matches it too.

**Every edit is verified by the engine** (`plan_edit.rs::apply_edit`) after being applied to a copy of the
plan: the clicked item must really end up as asked, otherwise the click is refused with a reason — an
ancestor excluded by a marker file, a pattern in an exclude file (applied after the plan's own patterns,
so a `!` can't override it; the message names file and line), a case-insensitive rule (a separate list
`!` can't cancel), or a source root that patterns don't apply to. New rules are appended last so they take
precedence over the plan's earlier ones.

**Click twice.** Clicking an item and clicking it again returns the exclude list to where it started —
identical lines, comments included — whenever the first click *added* lines and deleted none: the second deletes exactly
what the tree wrote, comment and all. When the first click *deleted* lines (an include removing an exact
line, an exclude removing a `!P`), the second puts its rules back at the **end** of the list, so what is
guaranteed is the same included set (the same rules too when the deleted lines were at the end): a
hand-written line cannot return to its old position or regain a comment it never had.
`[*.txt, /s/a.txt]` + include `a.txt` is the documented case: the dead exact line goes and `!a.txt` is
added, so the second click yields an equivalent list without the dead line. A `P` / `!P` pair can
appear only when the `!P` isn't what includes the folder (a partly included folder after "Include
everything inside it", or scaffolding): the `!P` stays, the exact rule is appended after it, and the
next click deletes that exact line. A pattern from the dialog is not undone by clicking the item (that
would include it, not remove the pattern): delete it in the exclude list.

**Verification against restic.** Two `#[ignore]` harnesses (need restic; also in `restic-compat`).
`restic_confirms_every_edit_the_engine_offers` applies random clicks (the way the UI picks them, `ui_click`:
left clicks, "Exclude by pattern…", and "Include everything inside it" on a partly included folder or an
excluded folder — never on a file) to random hand-written plans and trees and checks with real dry runs, run both before and after the edit,
that the clicked item ended up as asked, the engine agrees on every node and **nothing outside P, its
subtree and the path to P changed** (skipped for any `exclude` pattern other than P's exact path — a name
pattern, say — which is meant to match other items; `held_to_its_target`). `restic_confirms_click_sequences` runs 2–6 clicks per case (about half of the later ones
the inverse click on the same node; a third of the plans carry a user comment) and checks the same after
every click, plus that only P's own lines were deleted with every other line surviving in order, plus
the click-twice rule per class: *Identical* (first click only appended), *SameRules* (only deleted lines at the end),
*SameSet* (hand-written line deleted elsewhere), *Pair* ("include everything" on a partly included
folder leaves it included, so the next click is no undo but an exclude: the `!P` stays and P's exact rule
is appended after it, or only `!P` lines are deleted when an older one was what included P), and
otherwise only that the inverse click succeeds and the item is in the state asked for — a refused inverse
fails the test. Run both after touching
plan_edit.rs: `cargo test restic_confirms -- --ignored --nocapture` (`RESTY_DIFF_CASES`,
`RESTY_DIFF_SEED`; the sequence test costs ~4–5 restic runs per case). Last run (restic 0.19.1, 300 cases per seed, seeds 3 and 7): 297 and 296 edits, 1,211 and
1,175 clicks in 300 sequences each (19 and 32 "include everything", 16 and 25 of them on a partly
included folder), 476 and 445 round trips (mostly *Identical*, then *SameRules*; 5 and 7 *Pair*), 20,974
and 18,613 nodes compared — zero disagreements, collateral changes, foreign-line deletions or refused
inverse clicks (the 3–5 "refused as impossible" per run are clicks the engine refuses with a reason,
e.g. "include everything" below an excluded folder, where `!P` can't help).

**CLI parity (`tree_export`, "Use with restic CLI…").** Everything the GUI generates is a plain
restic pattern, so the plan's rules render as the files and command a user would run by hand:
one ordered `restic-excludes.txt` (the flag patterns and every exclude file's lines, in the order
restic evaluates them — one list, so `!` works across all of it), `restic-iexcludes.txt` for
case-insensitive rules (restic keeps those in a separate list), `--exclude-if-present` /
`--exclude-caches` as flags, and the sources. In files restic expands `$VAR` on every line, so a
literal `$` is written `$$`. The repository and password are never written (they come from
`RESTIC_REPOSITORY`/`RESTIC_PASSWORD`), and non-inclusion settings (tags, bandwidth, compression)
aren't in the command. `restic_confirms_exported_files_match_the_gui_flags` (`#[ignore]`) checks
that `--exclude-file` gives the same included set as the GUI's `--exclude` flags. The one feature that
would break parity by construction is the deferred include-pattern resolution (a runtime-built file
list); don't add it without an export story.

## Status / roadmap

1. **Engine + tests (`plan_rules.rs`)** — done, including Windows path semantics (`PathStyle`);
   the Windows end-to-end run is delegated to the `restic-compat` workflow (not yet seen green).
2. **Read-only tree** — done (this section); keyboard navigation (arrow keys) and a filter box for
   huge folders are not yet.
3. Editing — done: the tree is the source editor (one-click add/remove, multi-step undo, hidden-files
   switch, Home, resizable); inside a source a click excludes or includes exactly the item and a
   right-click menu (also from the keyboard) holds the pattern dialog and "include everything inside it";
   edits are engine-verified, delete the rules they supersede instead of stacking `P` and `!P`, write
   `# Tree:` comments, are checked for collateral changes and click-twice round trips against real
   restic, and the CLI export is in. Still to do: copy-on-import of exclude files, a rules list that
   replaces the exclude editors, and (deferred) include patterns resolved to an explicit
   `--files-from-verbatim` list at backup time (restic has no include patterns).
   Known test gap: the layout tests (`tests/layout`) never render the tree or open its dialogs
   (`tree_supported` is unmocked in `mock.js`), so the click, menu and dialog behaviour is covered by the
   pure helpers' unit tests, the Rust/restic harnesses and manual passes only. Also unverified so far: the
   click-sequence harness has run only locally (restic 0.19.1, macOS), not yet on any `restic-compat` leg —
   notably restic 0.17.3 and Windows — and the menu's ContextMenu key / Shift+F10 route has been driven
   only with synthesized key events in desktop Chrome, never in the real webviews.
3b. Path-list files in the tree — done, read-only (above). Still to do: edit the list file from the
   tree (with a warning), and open it from the app.
4. Polish: size estimates, keyboard/accessibility; verifying on a real Windows machine.
