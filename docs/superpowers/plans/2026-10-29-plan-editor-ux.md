# Plan Editor UX Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the backup-plan editor: `Files | Settings` tabs (Settings = one scrolling page with a jump list), a persistent footer, one-click include/exclude in the tree with a right-click menu, click-twice-restores re-include rules, and `# Tree:` comments in exclude lists.

**Architecture:** The Rust edit engine (`plan_edit.rs`) gains line bookkeeping (a pure `lines.rs`), new edit kinds, a collateral-flip check and a "brought back" count (`scan.rs`); the restic differential test is extended to prove them. The frontend replaces `boxClick` with pure `leftClick`/`menuEntries`, adds a keyboard-operable `ContextMenu`, keeps comments in the editor, and restructures the page into header / tabs / content / footer with a Settings pane driven by pure scroll-spy and URL helpers.

**Tech Stack:** Rust (Tauri v2, `plan_rules` engine), React 19 + TypeScript, Tailwind v3, Vitest (node environment: pure-function tests only), restic 0.19.1 on `PATH` for the `#[ignore]` tests, headless Chrome for `npm run test:layout`.

**Spec:** `docs/superpowers/specs/2026-10-29-plan-editor-ux-design.md` (read it first; this plan only adds the decisions the spec leaves open).

## Global Constraints

- Work on branch `feat/plan-editor-ux`. Never push, open PRs, bump versions, or edit `package.json`/`Cargo.toml` versions.
- No new dependency and no new setting/toggle. `plan_rules.rs` and restic argument building (`build_exclude_args`, `build_backup_args`) are not touched.
- Tree comments are exactly one line, prefix `# Tree: ` (`TREE_PREFIX`), and are the only comments the tree ever deletes.
- A tree click may delete only: P's exact lines, a `!P` line, carve-out scaffolding, and tree comments naming P directly above a deleted line. Never a broad rule, an exclude-file line, or a user comment.
- Edits may flip only P, nodes inside P, and folders on the path to P (collateral rule). A capped scan still allows the edit.
- All path/pattern code goes through `PathStyle` (Unix | Windows); every new Rust helper gets a Windows-style test.
- UI conventions (CLAUDE.md): no `cursor-pointer`; theme-mapped colors only (`text-gray-50` not `text-white`, mapped amber/red/green/blue shades for bare text); free-text truncates (`truncate` + `title`); no `overflow-x-auto`.
- Commit messages are sentence case, no prefix (see `git log`). Each commit must pass: `npm run typecheck`, `npm run lint:all`, `npm run test:vite`, `npm run test:rust`; UI tasks also `npm run test:layout` (Chrome is installed).

## Review Focus

Inputs the spec implies but no obvious task exercises, most likely first; each is pinned by a test in the named task.

1. Paths with glob characters (`we[ird]`, `x*y`) and Windows spelling (`C:/s/we[[]ird]`) when matching a click's exact line — Task 1.
2. A path containing a newline/control character written into a comment (would inject a line into the rule list) — Task 1.
3. Undoing sibling carve-outs out of order (first-carved file first) must keep the second file included — Task 3.
4. Legacy lists from the old behaviour: `P` then `!P` pairs, `!P` twice, duplicate exact lines — Tasks 2–3.
5. Old or malformed URLs: `?tab=advanced`, `?tab=settings&section=nope`, `?section=tags` without a tab — Task 10.

## File Structure

| File | Responsibility |
|---|---|
| `src-tauri/src/commands/plan_edit/lines.rs` (new) | Pure rule-list bookkeeping: exact/re-include line lookup, tree comments, scaffolding recognition, line removal |
| `src-tauri/src/commands/plan_edit/scan.rs` (new) | Bounded scans: collateral-flip finder, "brought back" counter |
| `src-tauri/src/commands/plan_edit.rs` | `apply_edit`, the four edit kinds, `EditOutcome` |
| `src-tauri/src/commands/plan_edit/tests.rs` | Engine unit tests + restic differential tests |
| `src/lib/planTree.ts` | `leftClick`, `menuEntries` (replace `boxClick`) |
| `src/lib/planTreeEdit.ts` | `EditAction`/`EditOutcome` types, `hasExactRule`, `summarizeEdit` |
| `src/lib/menuNav.ts` (new) | `nextEnabled` — keyboard index math for menus |
| `src/lib/scrollSpy.ts` (new) | `activeSectionIndex` — which Settings section is current |
| `src/lib/planForm.ts` | Comment-keeping save; `PlanTab`/`SettingsSection`/`FormIssue`/URL model |
| `src/components/ContextMenu.tsx` | Keyboard operation (shared with DiffPage) |
| `src/components/PlanTreePreview.tsx`, `PlanTreeEditDialog.tsx` | Click/menu wiring; dialog reduced to pattern-exclude + add-source confirm |
| `src/components/plan/ExcludeCard.tsx` | Comment rows, keep-everything mode switch |
| `src/components/plan/{SettingsTab,SettingsNav,PlanFooter}.tsx` (new), `BandwidthCard.tsx`, `ResticOptionsCard.tsx` (new, split from `AdvancedTab.tsx`, which is deleted), `RetentionSection.tsx`, `TagsSection.tsx` (renamed from `RetentionTab`/`TagsTab`), `PlanHeader.tsx`, `PlanTabs.tsx` | Layout |
| `src/pages/BackupPlanEditPage.tsx` | Shell: flex column, URL state, issue navigation |
| `tests/layout/pages.html` | Add Settings routes |

---

### Task 1: Rule-list bookkeeping (`lines.rs`)

**Files:**
- Create: `src-tauri/src/commands/plan_edit/lines.rs` (tests in-file, `#[cfg(test)] mod tests`)
- Modify: `src-tauri/src/commands/plan_edit.rs` (add `mod lines;`)

**Interfaces:**
- Produces (all `pub(crate)`, in `lines.rs`):
  - `const TREE_PREFIX: &str = "# Tree: ";` and `enum Verb { Excluded, Included }` (`Clone, Copy, PartialEq, Eq, Debug`)
  - `fn exact_rule(style: PathStyle, path: &str) -> String` — `style.escape_glob(&style.clean(path))`
  - `fn exact_lines(style: PathStyle, lines: &[String], path: &str) -> Vec<usize>` — ascending indices of lines whose trimmed text equals `exact_rule`
  - `fn reinclude_lines(style: PathStyle, lines: &[String], path: &str) -> Vec<usize>` — trimmed text equals `"!" + exact_rule`
  - `fn tree_comment(verb: Verb, path: &str, note: Option<&str>) -> String` — `"# Tree: excluded <path>"` / `"# Tree: included <path>"`, plus `" — <note>"`; control characters in `path`/`note` become `?`
  - `fn comment_names(line: &str, verb: Verb, path: &str) -> bool` — line is `# Tree: <verb> <path>` followed by end of string or `" — "` (so `/s/a` does not match `/s/ab`)
  - `fn remove_with_comments(lines: &mut Vec<String>, remove: &[usize], path: &str)` — removes the indices, plus any line directly above a removed line that `comment_names` P with either verb
  - `fn is_scaffolding(style: PathStyle, lines: &[String], idx: usize) -> bool` — `lines[idx]` is `!A` directly followed by `A/*`, or some later pattern line is a negation under `A/`
  - `fn orphan_scaffolding(style: PathStyle, lines: &[String], path: &str) -> Vec<usize>` — with P's leaf already absent from `lines`: walking P's ancestors nearest-first, a level `A` is returned (indices of its adjacent `!A`, `A/*` pair) iff no remaining line is a negation strictly under `A/`; stop at the first level that fails

- [ ] **Step 1: Write the failing tests** (in `lines.rs`), exact assertions:
  - `exact_rule_escapes_per_platform`: Unix `/s/we[ird]/x*y` → `/s/we\[ird\]/x\*y`; Windows `C:/s/we[ird]` → `C:/s/we[[]ird]`.
  - `exact_lines_find_only_plain_exact_lines`: lines `["/s/a","!/s/a","# /s/a","/s/a/*"," /s/a ","/s/a"]`, path `/s/a` → `exact_lines == [0,4,5]`, `reinclude_lines == [1]`.
  - `tree_comments_are_one_line_and_match_only_their_own_path`: `tree_comment(Excluded,"/s/a\nb",None)` contains no `'\n'` and contains `/s/a?b`; `comment_names(&tree_comment(Included,"/s/a",Some("why")),Included,"/s/a")` true; same line vs `"/s/ab"` false; `"# Tree: nothing"` false.
  - `removal_takes_the_adjacent_tree_comment_only`: `["# my note","# Tree: excluded /s/a","/s/a","*.log"]` remove `[2]` → `["# my note","*.log"]`; `["# Tree: excluded /s/a","*.log","/s/a"]` remove `[2]` → comment stays.
  - `scaffolding_is_recognised`: `["/s/X","!/s/X","/s/X/*","!/s/X/a"]`: `is_scaffolding(…,1)` true, `(…,3)` false; `["*.tmp","!/s/t.tmp"]` idx 1 false.
  - `orphan_scaffolding_stops_at_a_level_still_in_use`: lines `["/s/X","!/s/X","/s/X/*","!/s/X/c","/s/X/c/*"]`, path `/s/X/c/d` → `[1,2,3,4]`; add `"!/s/X/b"` at index 5 → `[3,4]`.
- [ ] **Step 2: Run to verify failure:** `cargo test --manifest-path src-tauri/Cargo.toml plan_edit::lines` → FAIL (items undefined).
- [ ] **Step 3: Implement** the interfaces above in `lines.rs`. Nothing calls them until Task 2, so declare `mod lines;` with `#[cfg_attr(not(test), allow(dead_code))]` (clippy runs with `-D warnings`); Task 2 removes that attribute.
- [ ] **Step 4: Run** the same command → PASS; `npm run lint:rust` clean.
- [ ] **Step 5: Commit** `git add src-tauri/src/commands/plan_edit.rs src-tauri/src/commands/plan_edit/lines.rs && git commit -m "Add rule-list bookkeeping for tree edits (exact lines, tree comments, scaffolding)"`

---

### Task 2: Engine — including (left-click include, include-everything)

**Files:**
- Modify: `src-tauri/src/commands/plan_edit.rs`; Test: `src-tauri/src/commands/plan_edit/tests.rs`

**Interfaces:**
- Consumes: Task 1.
- Produces: `EditAction { kind: String, path: String, pattern: Option<String> }` (the `scope` field is removed); kinds `"exclude"` (pattern; unchanged semantics), `"excludeItem"` (Task 3), `"include"`, `"includeEverything"`. `EditOutcome` gains `removed_rules: Vec<String>` (serialized `removedRules`) and `brought_back: Option<BroughtBack>` (`struct BroughtBack { count: u64, capped: bool }`, camelCase, filled in Task 4; `None` here). `added_rules`/`removed_rules` list real patterns only, never comments. Test helpers: `include(path) -> EditAction`, `include_everything(path)`, `exclude_item(path)`, `patterns(lines: &[String]) -> Vec<&str>` (drops comment lines).

**`do_include(work, path, …)` decision order** (after the existing "already included" and `OutsideSources` add-source handling, which are unchanged):
1. P excluded at P itself (`v.at` is `None` or `== path`): delete `exact_lines(P)` newest-first, re-building the engine after each, until P is included → outcome is deletions only (`removed_rules` = the deleted patterns). If P is still excluded after all exact lines are gone (or there were none): the deleted lines stay deleted (they were dead), then append `tree_comment(Included, P, note)` and `!<exact_rule>`; `added_rules = ["!P"]`.
2. P excluded because ancestor `x = v.at ≠ path` is excluded: append comment + `carve_out_chain(style, x, path)` (existing, minimal from the highest excluded node). Exact lines of P are left alone.
3. Marker / exclude-file / case-insensitive reasons: refuse via the existing `explain` text. Verify at the end exactly as today.
`do_include_everything`: skip the "already included" guard; refuse only `OutsideSources` ("It isn't inside a backup source."); append comment + `!<exact_rule>`; exact lines stay.
Notes (`tree_comment` third argument): plain `!P` file — `a broader rule excludes it; this re-includes only this item`; folder — `… and everything inside it`; carve-out — `out of the excluded <x>: reopen it, exclude what is inside, bring this back`; include-everything — `and everything inside it that other rules exclude`. Deletions add no comment and call `remove_with_comments`.

- [ ] **Step 1: Write the failing tests** (table-driven, source `/s`, `included_after` true for P unless stated; "after" ignores comments via `patterns`):

| before | action | after (patterns) |
|---|---|---|
| `["/s/a/c-1.png"]` | include `/s/a/c-1.png` | `[]` |
| `["# Tree: excluded /s/a/c.png","/s/a/c.png"]` | include `/s/a/c.png` | `[]`, no comment lines left |
| `["/s/a/c.png","*.log","/s/a/c.png"]` | include `/s/a/c.png` | `["*.log"]` |
| `["/s/d/x.txt","/s/d"]` | include `/s/d` | `["/s/d/x.txt"]`; `/s/d/x.txt` still excluded, `/s/d/y.txt` included |
| `["*.txt","/s/a/f.txt"]` | include `/s/a/f.txt` | `["*.txt","!/s/a/f.txt"]`, `removed_rules == ["/s/a/f.txt"]` |
| `["*.tmp"]` | include `/s/t.tmp` | `["*.tmp","!/s/t.tmp"]` |
| `["node_modules"]` | include `/s/a/node_modules/lib/index.js` | the 5-rule chain (`!…/node_modules`, `…/*`, `!…/lib`, `…/lib/*`, `!…/lib/index.js`) |
| result of the row above | include `/s/a/node_modules/lib/other.js` | previous + `["!/s/a/node_modules/lib/other.js"]` only; `index.js` still included |
| same | include `/s/a/node_modules/other/z.js` | previous + 3 rules from `/s/a/node_modules/other` |
| `["/s/X","/s/X/a.txt"]` | include `/s/X/a.txt` | `["/s/X","/s/X/a.txt","!/s/X","/s/X/*","!/s/X/a.txt"]` |
| `["/s/d/x.txt","/s/d"]` | includeEverything `/s/d` | `[…,"!/s/d"]`; `/s/d/x.txt` included after |
| `["/s/d/x.txt"]` | includeEverything `/s/d` | `[…,"!/s/d"]` |

  Also: every appended group has `comment_names(line, Verb::Included, P)` directly above its first rule; deletion rows leave no tree comment for P; a Windows row (`C:/s`, `C:/s/we[ird]/f.txt` excluded by `*.txt` → `!C:/s/we[[]ird]/f.txt`); delete the obsolete `the_whole_folder_choice_…` test and the `scope` argument from existing `include(…)` calls; keep the marker / exclude-file / case-insensitive refusal tests.
- [ ] **Step 2: Run:** `cargo test --manifest-path src-tauri/Cargo.toml plan_edit::tests` → FAIL (compile errors, then assertions).
- [ ] **Step 3: Implement** in `plan_edit.rs`: change `EditAction`/`EditOutcome`, rewrite `do_include` per the decision order, add `do_include_everything`, dispatch `"includeEverything"` in `apply_edit`; remove the `cfg_attr` from Task 1's `mod lines;`. The engine rebuild-and-verify at the end of each path stays (`classify` + `judge_node`).
- [ ] **Step 4: Run** the Step 2 command and `npm run lint:rust` → PASS.
- [ ] **Step 5: Commit** `git commit -am "Include removes the exact exclude line first, else re-includes only the item; add include-everything"` (add new files if any).

---

### Task 3: Engine — excluding (left-click exclude, undo of re-includes and carve-outs)

**Files:** Modify `plan_edit.rs`; Test `plan_edit/tests.rs`.

**Interfaces:** Consumes Tasks 1–2. Produces kind `"excludeItem"` → `do_exclude_item(work, path, probe, read, getenv)`; kind `"exclude"` (pattern dialog) now prepends `tree_comment(Excluded, P, Some("with pattern <pattern>"))`.

**`do_exclude_item` decision order** (P must be included, else the existing "already excluded" errors; a source path delegates to the existing `do_exclude` with the exact rule, which removes the source):
1. Lone re-includes: `reinclude_lines(P)` minus `is_scaffolding` ones; delete newest-first until P is excluded (engine-verified), removing their tree comments (`remove_with_comments`). Then, in the same edit, also delete `orphan_scaffolding(remaining lines, P)` so undoing the last carve-out leaves no empty folder behind (it returns nothing when a level is still in use or no scaffolding exists). Done if P is excluded.
2. Otherwise append `tree_comment(Excluded, P, None)` + `exact_rule(P)`; verify P excluded (if not, the existing "doesn't exclude it: <reason>" error).

- [ ] **Step 1: Write the failing tests** (same helpers; "after" via `patterns`):

| before | action | after |
|---|---|---|
| `["*.log"]` | excludeItem `/s/a/c-1.png` | `["*.log","/s/a/c-1.png"]`, comment above it |
| `["*.tmp","# Tree: included /s/t.tmp — why","!/s/t.tmp"]` | excludeItem `/s/t.tmp` | `["*.tmp"]`, no tree comment left |
| `["/s/a/f.txt","!/s/a/f.txt"]` (legacy pair) | excludeItem `/s/a/f.txt` | `["/s/a/f.txt"]` |
| `["*.tmp","!/s/t.tmp","!/s/t.tmp"]` | excludeItem `/s/t.tmp` | `["*.tmp"]` |
| `["# my note","*.tmp","!/s/t.tmp"]` | excludeItem `/s/t.tmp` | `["# my note","*.tmp"]` (user comment kept) |
| the `node_modules` carve-out of `lib/index.js` (the 5-rule result in Task 2) | excludeItem `index.js` | `["node_modules"]`, no tree comments |
| carve-outs of `lib/index.js` then `lib/other.js` | excludeItem `index.js` (the first-carved) | `other.js` still included, `index.js` excluded, scaffolding for `X` and `lib` kept |
| same two carve-outs | excludeItem `other.js`, then `index.js` | `["node_modules"]` |
| carve-out of `index.js` | excludeItem `/s/a/node_modules` (partial folder; its `!X` is scaffolding) | previous list + `…/node_modules` exact line; then `include /s/a/node_modules` returns the list to exactly the carve-out result |

  Round-trip tests: for each row of Task 2's table where the include *appended* rules, `excludeItem` on the same P returns the starting list exactly (text, comments included). `exclude(path, pattern)` still passes its existing tests with a comment line added (update full-list assertions). Windows: `C:/s/we[ird]/f.txt` → exact line `C:/s/we[[]ird]/f.txt`.
- [ ] **Step 2: Run:** `cargo test --manifest-path src-tauri/Cargo.toml plan_edit::tests` → FAIL.
- [ ] **Step 3: Implement** `do_exclude_item`; dispatch `"excludeItem"`; add the pattern-exclude comment.
- [ ] **Step 4: Run** Step 2 command + `npm run lint:rust` → PASS.
- [ ] **Step 5: Commit** `git commit -am "Exclude by exact path; undo re-includes and carve-outs instead of stacking rules"`

---

### Task 4: Engine — collateral check and "brought back" count (`scan.rs`)

**Files:** Create `src-tauri/src/commands/plan_edit/scan.rs`; Modify `plan_edit.rs`; Test: in-file tests using a real temp dir (reuse the `scratch(label)` helper in `plan_edit/tests.rs` or `std::env::temp_dir()` + unique name, removed at the end).

**Interfaces:**
- Consumes: `ScanLimits`, `MatchPreview` from `plan_tree.rs`; `Engine`, `Probe` from `plan_rules.rs`.
- Produces (`pub(crate)`):
  - `fn find_collateral(before: &Engine, after: &Engine, probe: &dyn Probe, target: &str, limits: ScanLimits) -> Option<String>` — breadth-first over `after.sources()` (entering any directory that either engine does not reject), returns the first path whose `classify(..).included` differs between the engines and which is neither `target`, inside `target`, nor an ancestor of `target`; `None` if none found or the scan ran out of budget
  - `fn count_brought_back(others: &Engine, probe: &dyn Probe, folder: &str, limits: ScanLimits) -> MatchPreview` — items inside `folder` that `others` judges excluded (a rejected folder counts once and is not entered), capped like `count_matches`
- `apply_edit` calls `find_collateral` for kinds `excludeItem`, `include` (not the add-source path) and `includeEverything`, with `COLLATERAL_LIMITS` (750 ms, 2,000,000 entries — it runs on every click, so it must stay quick; a capped scan is allowed); a hit returns `Err("This change would also change <path>, so it wasn't made.")`. For include/includeEverything edits whose appended rules end in a `!` rule on a **directory** P (`std::fs::metadata`), `brought_back = Some(BroughtBack{count, capped})` when `count > 0`, where `others` = engine over the plan's *before* patterns minus those that `pattern_matches_styled` P itself.

- [ ] **Step 1: Write the failing tests:**
  - `collateral_finds_a_flipped_sibling`: temp dir with `a.txt`, `b.txt`; `before` = no rules, `after` = rule `<tmp>/b.txt`; target `<tmp>/a.txt` → `Some(<tmp>/b.txt)`; target `<tmp>/b.txt` → `None`; target = `<tmp>` → `None`.
  - `normal_edits_have_no_collateral`: on a temp tree (`X/a.txt`, `X/b.txt`, `X/sub/c.txt`, `other/e.png`) with rule `<tmp>/X`, `apply_edit` include `<tmp>/X/a.txt` succeeds and a second include of `<tmp>/X/sub/c.txt` keeps `a.txt` included (`included_after`).
  - `brought_back_counts_what_other_rules_hide`: tree `sub/c.txt`, `sub/e.tmp`, `keep.txt`; rules `["*.tmp","<tmp>/sub"]`, include `<tmp>/sub` → `brought_back == Some{count:1, capped:false}`; rules `["<tmp>/sub"]` → `None`; includeEverything on partial folder with `["<tmp>/sub/c.txt"]` → `count 1`.
- [ ] **Step 2: Run:** `cargo test --manifest-path src-tauri/Cargo.toml plan_edit::scan plan_edit::tests` → FAIL.
- [ ] **Step 3: Implement** `scan.rs`, wire into `apply_edit`.
- [ ] **Step 4: Run** Step 2 command, then `npm run test:rust`, `npm run lint:rust` → PASS.
- [ ] **Step 5: Commit** `git add -A src-tauri && git commit -m "Refuse tree edits that change anything but the clicked item; count items a folder re-include brings back"`

---

### Task 5: Prove it against real restic

**Files:** Modify `src-tauri/src/commands/plan_edit/tests.rs` (`restic_confirms_every_edit_the_engine_offers`; add `restic_confirms_click_sequences`, both `#[ignore]`).

**Interfaces:** Consumes the whole engine. Existing helpers `restic_available`, `scratch`, `grow`, `restic_included`, `canon`, `esc`.

- [ ] **Step 1: Extend the single-edit test.** Choose actions as the UI will: included target → `excludeItem` (75 %) or pattern `exclude` (25 %); excluded target → `include` or (25 %) `includeEverything`. Add a restic run **before** the edit (`restic_included(&repo, &plan.sources, &before_args, &base)`) and assert: for every existing node not equal to, inside, or an ancestor of the target, `before.contains(n) == after.contains(n)`.
- [ ] **Step 2: Add `restic_confirms_click_sequences`:** per case, 2–6 random clicks on random nodes, feeding each outcome's `sources`/`excludes` into the next plan, including clicking the same node twice in a row; after each click assert restic agrees with the engine on every node and the collateral rule above. Round trip: after a click on P apply the inverse click on P and assert the rule list equals the one before the first click, **except** when the first click was an `include` that deleted lines (`removed_rules` non-empty): then assert only that restic's included set equals the starting set, and only if no `!` line follows a deleted line (a hand-written mid-list line cannot be put back in place — see Task 13). Also assert every line that existed before and is not P's exact line, a `!P`, scaffolding or a tree comment is still present, in order.
- [ ] **Step 3: Run** `cd src-tauri && cargo test restic_confirms -- --ignored --nocapture` → prints `restic confirmed N edits` and passes. Run again with `RESTY_DIFF_CASES=300 RESTY_DIFF_SEED=7` and `RESTY_DIFF_SEED=11`. Any failure is an engine bug: fix it in `plan_edit.rs`/`lines.rs`/`scan.rs` (add a unit test for the minimal failing case first), never weaken the assertion.
- [ ] **Step 4: Commit** `git commit -am "Check tree edits against restic for collateral changes and click-twice round trips"`

---

### Task 6: Frontend click model (pure)

**Files:** Modify `src/lib/planTree.ts`, `src/lib/planTreeEdit.ts`, `src/lib/invoke.ts` (types only if needed); Test `src/lib/planTree.test.ts`, `src/lib/planTreeEdit.test.ts` (create if absent).

**Interfaces:**
- Produces in `planTree.ts` (replacing `boxClick`/`BoxClick`):
  - `type LeftClick = "removeSource" | "addSource" | "exclude" | "include"`; `leftClick(isSource: boolean, verdict: NodeVerdict | undefined): LeftClick` — source → `removeSource`; excluded with reason `outsideSources` → `addSource`; other excluded → `include`; included or no verdict → `exclude`
  - `type MenuAction = LeftClick | "excludeByPattern" | "includeEverything"`; `interface MenuEntry { action: MenuAction; label: string }`
  - `menuEntries(a: { isSource: boolean; verdict: NodeVerdict | undefined; state: DisplayState; kind: SourceKind; exactRule: boolean }): MenuEntry[]` — first entry's action always equals `leftClick(...)`. Included: `Exclude`, `Exclude by pattern…`, plus `Include everything inside it` when `state === "partial"`. Excluded inside a source: `Include`, plus `Include, plus everything inside it that other rules exclude` only when `kind === "dir" && exactRule`. Source / outside-source: one entry, `Stop backing up this folder|file` / `Back up this folder|file`.
- Produces in `planTreeEdit.ts`: `EditAction { kind: "exclude" | "excludeItem" | "include" | "includeEverything"; path: string; pattern?: string }` (`scope?: "item" | "folder"` stays as a deprecated optional field, and `boxClick`/`BoxClick` stay exported, only until Task 8 deletes them with their last callers); `EditOutcome` gains `removedRules: string[]` and `broughtBack?: { count: number; capped: boolean } | null`; `hasExactRule(excludes: string[], path: string, windows: boolean): boolean` (trimmed line equals `escapeGlob(path, windows)`; `!` and `#` lines never count); `summarizeEdit` adds `removed N rule(s)` and, when `broughtBack`, `also brought back N items other rules exclude` (`at least N` when `capped`).

- [ ] **Step 1: Write the failing tests:** `leftClick` over (source, included, excluded-inside, excluded-outside, undefined verdict); `menuEntries` for each row of the spec's right-click table (assert action lists, e.g. partial dir → `["exclude","excludeByPattern","includeEverything"]`; excluded dir with exact rule → `["include","includeEverything"]`; excluded file → `["include"]`; first entry equals `leftClick`); `hasExactRule`: `["*.png","/s/a/c-1.png"]` true, only `["!/s/a/c-1.png"]` false, `["# /s/a/c-1.png"]` false, Windows `C:/s/we[[]ird]/f` true for `C:/s/we[ird]/f`; `summarizeEdit({…removedRules:["x"], broughtBack:{count:12,capped:false}})` contains `also brought back 12 items other rules exclude`, capped → `at least 12`.
- [ ] **Step 2: Run** `npx vitest run src/lib/planTree.test.ts src/lib/planTreeEdit.test.ts` → FAIL.
- [ ] **Step 3: Implement** (additive; leave `boxClick` and its tests until Task 8).
- [ ] **Step 4: Run** the Step 2 command, then `npm run typecheck && npm run lint` → PASS.
- [ ] **Step 5: Commit** `git commit -am "Add pure left-click and right-click menu decisions for the plan tree"`

---

### Task 7: Keyboard-operable `ContextMenu`

**Files:** Create `src/lib/menuNav.ts`, `src/lib/menuNav.test.ts`; Modify `src/components/ContextMenu.tsx`.

**Interfaces:** Produces `nextEnabled(key: string, current: number, enabled: boolean[]): number | null` — `ArrowDown`/`ArrowUp` move to the next/previous enabled index with wrap, `Home`/`End` go to first/last enabled, any other key or no enabled item → `null`. `ContextMenu` props unchanged.

- [ ] **Step 1: Failing tests:** `nextEnabled("ArrowDown",0,[true,false,true])===2`; wraps `("ArrowDown",2,…)===0`; `("ArrowUp",0,…)===2`; `("Home",2,…)===0`; `("End",0,…)===2`; `("a",0,…)===null`; all disabled → `null`.
- [ ] **Step 2: Run** `npx vitest run src/lib/menuNav.test.ts` → FAIL.
- [ ] **Step 3: Implement** `nextEnabled`; in `ContextMenu.tsx`: container `role="menu"`, buttons `role="menuitem" tabIndex={-1}`, focus the first enabled item on mount, `onKeyDown` on the container uses `nextEnabled` to move focus, Escape (existing document listener) additionally restores focus to the element focused when the menu opened. Mouse behaviour and props unchanged.
- [ ] **Step 4: Run** `npx vitest run src/lib/menuNav.test.ts && npm run typecheck && npm run lint` → PASS. Manual smoke (record in the final report): DiffPage right-click still opens, clicks work, ↓/Enter/Esc work.
- [ ] **Step 5: Commit** `git add src/lib/menuNav.ts src/lib/menuNav.test.ts src/components/ContextMenu.tsx && git commit -m "Make the shared context menu keyboard-operable"`

---

### Task 8: Tree UI — left-click, right-click menu, dialog reduction

**Files:** Modify `src/components/PlanTreePreview.tsx`, `src/components/PlanTreeEditDialog.tsx`.

**Interfaces:** Consumes Tasks 6–7 and the existing `treePlanEdit`, `escapeGlob`, `displayState`. Produces nothing new for later tasks.

- [ ] **Step 1: Dialog.** `EditTarget` drops `isSource`; `modeFor` becomes included → `"exclude"`, else `"include"`; delete the `removeSource` mode and the `includeScope` radio group; `include` mode is only the add-source confirmation (it sends `{ kind: "include", path }`); keep the outcome listing (`addedSources`, `removedSources`, `addedRules`). Titles: `Exclude “name”` / `Back up “name”`.
- [ ] **Step 2: Actions.** Replace `onBox` with `run(path: string, kind: SourceKind, action: MenuAction)`: `exclude` → `treePlanEdit(fullRules, {kind:"excludeItem", path})`; `include` → `{kind:"include", path}`; `includeEverything` → `{kind:"includeEverything", path}`; `removeSource` → existing `{kind:"exclude", path, pattern: escapeGlob(path, windows)}`; `addSource` → existing flow (open the dialog when `removedSources.length > 0 || addedRules.length > 0`, else apply); `excludeByPattern` → `openEdit`. Keep the `settled`/`busy` guards and the path-list-file message. Left-click = `run(path, kind, leftClick(...))`.
- [ ] **Step 3: Menu.** `onContextMenu` on each row's `treeitem` div (`preventDefault`, store `{x, y, path, kind}`); the box button also opens it at its bounding rect for `ContextMenu` key and `Shift+F10`; render `<ContextMenu items={menuEntries({…, exactRule: hasExactRule(rules.excludes, path, windows)}).map(e => ({label: e.label, onClick: () => run(path, kind, e.action)}))} />`. Update `boxTitle` (`Exclude` / `Include` / source labels) and the hint under the tree: `Click to include or exclude · Right-click for more options`.
- [ ] **Step 4: Clean up Task 6's leftovers:** delete `boxClick`, `BoxClick` and their tests from `planTree.ts`/`planTree.test.ts`, and the deprecated `scope` field from `EditAction`.
- [ ] **Step 5: Verify:** `npm run typecheck && npm run lint && npm run test:vite && npm run test:layout` → PASS.
- [ ] **Step 6: Commit** `git commit -am "Tree: one click excludes or includes the item; right-click for the advanced choices"`

---

### Task 9: Comments in the editor

**Files:** Modify `src/lib/planForm.ts`, `src/components/plan/ExcludeCard.tsx`; Test `src/lib/planForm.test.ts`.

**Interfaces:** Produces in `planForm.ts`: `isCommentLine(line: string): boolean` (trimmed starts with `#`); `expertLines(text: string): string[]` (replaces `expertPatterns`: trimmed, blank lines dropped, comments kept); `savedExcludes` uses it.

- [ ] **Step 1: Failing tests:** rewrite "saves expert text…" to expect `["node_modules","# note","!keep","*.log"]` for `"  node_modules \n# note\n\n!keep\n*.log"` and `expertLines("a\n#b\n  \n c ") == ["a","#b","c"]`; simple mode keeps `["# why","!x","y","!x"]` as is; `formFromPlan` then `buildPlan` on a plan whose `excludes` contain comments and repeats returns them unchanged; `isCommentLine("  # x")` true.
- [ ] **Step 2: Run** `npx vitest run src/lib/planForm.test.ts` → FAIL.
- [ ] **Step 3: Implement.** `ExcludeCard`: `switchMode` expert→simple sets `excludeItems: expertLines(excludeText)` (no `Set`); the simple list renders comment lines as muted italic rows (`text-gray-500`, same × button, no pattern styling); `addExclude` accepts `# …` and allows duplicate comments (non-comment duplicates still rejected); help text mentions comments are kept.
- [ ] **Step 4: Run** `npm run typecheck && npm run lint && npm run test:vite && npm run test:layout` → PASS.
- [ ] **Step 5: Commit** `git commit -am "Keep comments in exclude patterns; stop de-duplicating when leaving Expert"`

---

### Task 10: Editor model — tabs, Settings sections, issues, URL (pure)

**Files:** Modify `src/lib/planForm.ts`; Test `src/lib/planForm.test.ts`.

**Interfaces:** Produces (all exported from `planForm.ts`): `PlanTab = "files" | "settings"`, `PLAN_TABS`, `PLAN_TAB_LABELS` (`Files`, `Settings`), `isPlanTab`; `SettingsSection = "retention" | "tags" | "bandwidth" | "restic" | "webhooks"`, `SETTINGS_SECTIONS`, `SETTINGS_SECTION_LABELS` (`Retention`, `Tags`, `Bandwidth`, `Restic options`, `Webhooks`), `isSettingsSection`; `FormIssue { where: "header" | "files" | SettingsSection; message: string }`; `issueTab(where: FormIssue["where"]): PlanTab | null` (`files`→`files`, a section→`settings`, `header`→`null`); `planLocation(tab: string | null, section: string | null): { tab: PlanTab; section: SettingsSection | null }`; `sectionIsCustom(f: PlanFormState, s: SettingsSection): boolean` (Retention: any field set; Tags: ≥1 tag; Bandwidth: a limit set; Restic options: pack size or a Windows flag; Webhooks: ≥1); `settingsIsCustom(f)`. Removes `retentionSummary`, `advancedIsCustom`.

- [ ] **Step 1: Failing tests:** validation `where` values: list-file-without-tag → `"tags"`; bad webhook URL → `"webhooks"`; bad pack size → `"restic"`; bad retention → `"retention"`; others unchanged. `planLocation`: `("settings","webhooks")`→settings/webhooks; `("retention",null)`→settings/retention; `("tags",null)`→settings/tags; `("advanced",null)`→settings/bandwidth; `("settings","nope")`→settings/null; `(null,"tags")`→files/null; `("files","tags")`→files/null; `("zzz",null)`→files/null. `issueTab` for each kind. `sectionIsCustom` per section (reuse the existing `advancedIsCustom` fixtures, split by section); `settingsIsCustom` true if any.
- [ ] **Step 2: Run** `npx vitest run src/lib/planForm.test.ts` → FAIL.
- [ ] **Step 3: Implement**; delete the removed functions and their tests.
- [ ] **Step 4: Run** `npx vitest run src/lib/planForm.test.ts` → PASS. `npm run typecheck` fails in the components until Task 12, so **Tasks 10–12 are one commit**, made at the end of Task 12.

---

### Task 11: Settings pane and jump list

**Files:** Create `src/lib/scrollSpy.ts`, `src/lib/scrollSpy.test.ts`, `src/components/plan/SettingsTab.tsx`, `SettingsNav.tsx`, `BandwidthCard.tsx`, `ResticOptionsCard.tsx`; `git mv` `RetentionTab.tsx`→`RetentionSection.tsx`, `TagsTab.tsx`→`TagsSection.tsx`; delete `AdvancedTab.tsx` (its Bandwidth card, Restic-options card and the `WebhooksCard` use move to the new files / `SettingsTab`).

**Interfaces:**
- Produces `activeSectionIndex(tops: number[], scrollTop: number, viewportHeight: number, scrollHeight: number, slack = 40): number` — last index whose `top - slack <= scrollTop`; the last index when scrolled to the bottom (`scrollTop + viewportHeight >= scrollHeight - 2`); `0` for empty `tops` entries before the first.
- Produces `SettingsTab(props: FormProps & { seededFromDefault: boolean; requested: SettingsSection | null; onJump: (s: SettingsSection) => void; alertSection: SettingsSection | null })` — one scroll container (`position: relative`, bottom padding ≥ 220 px) holding five `<section id="plan-section-<s>">` with an `h2` of `SETTINGS_SECTION_LABELS[s]` and the existing card content (cards drop their own titles, keep help text); `SettingsNav(props: { active: SettingsSection; custom: (s) => boolean; alert: SettingsSection | null; onJump: (s) => void })` — `<nav aria-label="Settings sections">`, buttons with `aria-current` on the active one, thin accent bar, blue dot for custom, red dot for alert, no summaries.
- Behaviour: clicking a name scrolls the pane to `section.offsetTop - 18` (`behavior: "auto"` when `prefers-reduced-motion`) and calls `onJump`; a changed `requested` prop scrolls the same way without calling `onJump`; scrolling (rAF-throttled) sets the active section via `activeSectionIndex`.

- [ ] **Step 1: Failing tests** (`scrollSpy.test.ts`): tops `[0,300,700]`: scrollTop 0 → 0; 270 → 1 (slack); 650 → 2; scrolled to bottom with scrollTop 500, viewport 400, height 900 → 2; scrollTop 299, viewport 400, height 2000 → 0 … 1 per the slack rule (assert the exact boundary 260 → 1, 259 → 0).
- [ ] **Step 2: Run** `npx vitest run src/lib/scrollSpy.test.ts` → FAIL.
- [ ] **Step 3: Implement** `scrollSpy.ts`, then the components per the interfaces; reuse `CARD`, `RetentionFields`, `Input`, `WebhooksCard` unchanged.
- [ ] **Step 4: Run** `npx vitest run src/lib/scrollSpy.test.ts` → PASS. No commit yet (see Task 10, Step 4).

---

### Task 12: Page shell, header, tabs and footer

**Files:** Create `src/components/plan/PlanFooter.tsx`; Modify `PlanHeader.tsx`, `PlanTabs.tsx`, `src/pages/BackupPlanEditPage.tsx`, `tests/layout/pages.html`.

**Interfaces:** Consumes Tasks 10–11. Produces `PlanFooter(props: { isNew: boolean; saving: boolean; error: string; onSave: () => void; onCancel: () => void; onDelete: () => void })` — Delete (danger, `!isNew` only) far left, spacer, error message (`role="alert"`, `truncate`, `title={error}`), Cancel, `Save Changes` / `Create Plan`. `PlanHeader` loses its buttons and `isNew/saving/onSave/onCancel/onDelete` props and is no longer sticky. `PlanTabs` renders the two tabs with `info: Partial<Record<PlanTab, { dot?: boolean; alert?: boolean }>>` (no summaries).

- [ ] **Step 1: Implement.** Page root `flex flex-col h-full`: `PlanHeader`, `PlanTabs`, a `flex-1 min-h-0` content region, `PlanFooter`. Files panel: `h-full overflow-y-auto px-6 pt-4 pb-6` (mounted once visited, `hidden` when inactive — tree state survives, `active` prop as today); Settings panel: `SettingsTab` (lazy-mounted, kept). URL: `planLocation(params.get("tab"), params.get("section"))`; tab clicks write `tab`; `onJump` writes `tab=settings&section=<s>` (`replace: true`). A failed save: footer shows the message, `setIssue(problem)`, then `header` → focus name/repository; `files` → select Files; a section → `tab=settings&section=<where>`; keep the issue set after navigating (as today), clear it when the user edits or jumps to that section. Tab dots: `settings: { dot: settingsIsCustom(form), alert: issueTab(issue.where)==="settings" }`, `files: { alert }`. The old top error banner is removed (errors from load/delete also go to the footer). Delete modal unchanged.
- [ ] **Step 2: Layout test routes.** Add `/backup-plans/plan-1?tab=settings` and `/backup-plans/new?tab=settings` to the route list in `tests/layout/pages.html` (the mock already has a 120-character plan name).
- [ ] **Step 3: Verify:** `npm run typecheck && npm run lint && npm run test:vite && LAYOUT_REQUIRE_BROWSER=1 npm run test:layout` → PASS (no horizontal scroll, footer buttons unclipped at 1024). Fix class-level problems found (truncation, `min-w-0`) rather than relaxing the harness.
- [ ] **Step 4: Commit** (carries Tasks 10, 11 and 12) `git add -A src tests && git commit -m "Plan editor: Files | Settings tabs, persistent footer, Settings jump list"`

---

### Task 13: Docs and final verification

**Files:** Modify `docs/backup-plan-tree.md`, `docs/frontend.md`, `docs/decisions.md`, `CLAUDE.md`, `docs/superpowers/specs/2026-10-29-plan-editor-ux-design.md` (§5 item 4 refinement).

- [ ] **Step 1: Update docs** per spec §6: `backup-plan-tree.md` ("Clicking a box", "Editing from the tree", roadmap, the F1–F6 facts, the decision orders from Tasks 2–3); `frontend.md` (`BackupPlanEditPage` structure paragraph: tabs, footer, jump list, URL state, menu, comments, `ContextMenu` keyboard); `decisions.md` (re-include never touches broad rules; `!P` semantics and accepted broad-folder limit; no whole-folder option; tree comments; collateral check); `CLAUDE.md` (the `plan/` structure line, `BackupPlanEditPage` line, `PlanTreePreview`/`ContextMenu` lines, one Settled-decisions bullet, Testing line if needed). Spec §5 item 4: say the click-twice identity guarantee holds for lines the tree itself added, and for deleted hand-written lines only as an equivalent included set (position cannot be restored).
- [ ] **Step 2: Full verification:** `npm run typecheck && npm run lint:all && npm run test:all`, then `cd src-tauri && cargo test restic_confirms -- --ignored --nocapture` with `RESTY_DIFF_CASES=300`; all green. Report anything skipped.
- [ ] **Step 3: Review the branch diff** (`git diff feat/files-tab-tree...HEAD --stat`); confirm no `package.json`/`Cargo.toml` version change and nothing outside the File Structure table.
- [ ] **Step 4: Commit** `git commit -am "Document the plan editor rework"`
- [ ] **Step 5: Hand-off checklist for the user** (cannot be automated): in `npm run tauri dev`, check left/right-click on a source, an included file, a partial folder, an excluded file, a file inside an excluded folder; click-click returns the exclude list to its start (Expert view); comments appear and survive Save/reopen; Settings jump list and footer at the minimum window size; DiffPage context menu still works.
