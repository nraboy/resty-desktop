# Plan editor UX rework — design

Date: 2026-10-29 · Branch: `feat/plan-editor-ux` (off `feat/files-tab-tree`, not yet merged to main)

Read first: `CLAUDE.md` (index + settled decisions), `docs/backup-plan-tree.md` (the rule engine and the
tree), `docs/frontend.md` § `BackupPlanEditPage.tsx`. Terms: **P** is the item the user clicked; a
**source** is a folder/file the plan backs up; "inside a source" means below a source root.

## Why

Feedback from people using the plan editor after the file-tree work (`feat/files-tab-tree`):

1. Four tabs (Files | Retention | Advanced | Tags) are disliked, but one long scrolling page is painful too.
2. Save/Cancel/Delete in the sticky header is not wanted there; they belong at the bottom.
3. Inside a source, clicking a tree box opens a dialog asking for a pattern scope. It is annoying: people
   want the click to mean "this exact file/folder".
4. Excluding `c-1.png` then including it again leaves `/p/c-1.png` **and** `!/p/c-1.png` in the exclude
   list. Un-excluding should just remove the exclude.
5. Complicated rule lists (carve-outs especially) are hard to read; rules should be able to carry comments.

Three pieces, one branch: **layout** (§1), **tree clicks and rule bookkeeping** (§2–§3), **comments** (§4).

## Goals

- Plan editor is `Files | Settings`; Settings is one scrolling page with a quiet jump list, like VS Code's.
- Save / Cancel / Delete live in a persistent footer on both tabs.
- Inside a source: left-click excludes/includes exactly the clicked item, no dialog; right-click opens a
  menu with the advanced choices (today's pattern dialog stays, behind the menu).
- Including an item never edits or deletes a broad rule; it removes the user's own exact rule if there is
  one, otherwise adds the specific `!P`. Clicking twice returns the list to where it started.
- Exclude lists can hold comments; the tree writes a one-line comment above every rule group it adds.

## Non-goals and anti-goals

- No change to how restic's arguments are built, to source add/remove clicks, to the exclude-pattern
  dialog's scopes, to Simple/Expert exclude modes, to path-list/exclude files, or to `plan_rules.rs`.
- No "include the whole excluded folder" option on files (explicitly rejected: if that were the intent the
  user would click the folder).
- No new dependency. No new setting/toggle for any of this.
- Anti-goals — each of these makes the work a failure even if tests pass: a tree click that deletes or
  rewrites a broad rule (`*.png`, `node_modules`, an exclude-file line); a `P` / `!P` pair appearing from a
  plain click-click; an edit that silently changes the included state of anything other than P, what is
  inside P, and the folders on the path to P; comments changing what restic backs up.

## Facts verified against restic 0.19.1 (dry runs; the differential tests are the authority)

Rule lists below are in order; `X` a folder, files inside it. `restic backup --dry-run -vv --exclude-file`.

- **F1** `!P` brings back P and *everything below it*, overriding earlier excludes of items below P:
  `[*.tmp, sub, !sub]` backs up `sub/*.tmp`; `[sub/c.txt, sub, !sub]` backs up `sub/c.txt`.
- **F2** Deleting P's exact line leaves other rules about P's contents in force:
  `[sub/c.txt, sub]` → delete `sub` → `c.txt` stays excluded, siblings return.
- **F3** Carve-out chain for a file directly in excluded `X`: `[X, !X, X/*, !X/a]` backs up only `a`
  (inside the folder `X` itself, which is now backed up). Re-appending the *full* chain for a second file
  (`…, !X, X/*, !X/b`) **drops `a`** — a naive implementation clobbers earlier carve-outs. The engine
  avoids this today by starting the chain at the highest still-excluded node (for `b` that is `X/b`
  itself, so only `!X/b` is added). Keep that property.
- **F4** Scaffolding without a leaf (`[X, !X, X/*]`) backs up an *empty folder* `X`; the plan is not back to
  how it started.
- **F5** With a broad rule also excluding P (`[*.txt, a.txt]`), deleting the exact line changes nothing.
- **F6** `[X, !X, X/*, !X/a]` then `X` appended excludes everything again; then `!X` appended brings back
  *all* of X (F1).

## 1. Layout

**Shell.** `BackupPlanEditPage` keeps owning form state, load, save, delete (`lib/planForm.ts` stays the
pure rules). `PlanTab` becomes `"files" | "settings"`.

**Header (top, fixed).** Editable plan name and repository select with their amber notes, as today, but
**no buttons**. Then the tab strip: `Files`, `Settings`. A blue dot on `Settings` when any section below is
set away from its default; a red dot when the last save failed in one.

**Footer (bottom, fixed, both tabs).** Delete (existing plans only; danger, far left), spacer, the error
message of a failed save (`role="alert"`, replaces today's top banner; long messages must wrap or truncate
with the full text reachable), Cancel, Save Changes / Create Plan. Header, tab strip and footer stay
visible; only the content between them scrolls. The app's scroll container is `<main className="flex-1
overflow-y-auto">` in `App.tsx`; the page becomes a full-height flex column. (my call: Delete left, error in
footer)

**Files tab.** Content unchanged: tree, `ListFilesCard`, `FdaNotice`, `ExcludeCard`, `MarkersCard`
(`SourcesCard` fallback as today). It stays mounted when Settings is shown (tree state, undo history, scans
survive; preview pauses via `active`).

**Settings tab.** One scrolling pane containing, in order, sections **Retention**, **Tags**,
**Bandwidth**, **Restic options**, **Webhooks** (the existing cards/fields, regrouped: Bandwidth and the
pack-size/Windows flags are today's two Advanced cards). Left of it, a **quiet jump list**: names only, no
cards or summaries (the old one-line summaries go), a thin accent bar on the current section, small blue dot
= section set away from default, red dot = last save failed here. Clicking a name scrolls the pane to that
section; scrolling moves the highlight (scroll-spy). Enough bottom padding that the last section can reach
the top. Smooth scrolling honours `prefers-reduced-motion`. Dot rule, for consistency (my call): Retention
= any field set; Tags = ≥1 tag; Bandwidth = a limit set; Restic options = pack size or a Windows flag set;
Webhooks = ≥1.

**State in the URL.** `?tab=files|settings` and (Settings) `&section=retention|tags|bandwidth|restic|webhooks`,
written on jump, not on every scroll tick. Old links `?tab=retention|advanced|tags` open Settings at the
matching section (my call).

**Validation.** `FormIssue.where` names `"header"`, `"files"`, or a Settings section. A failed save puts the
message in the footer, switches to the right tab, scrolls to the section, and shows the red dots; editing or
saving again clears them (same acknowledge behaviour as today).

**Accessibility.** Jump list is a `<nav aria-label>` of buttons with `aria-current` on the active one; the
tab strip keeps its tablist semantics with two tabs. Layout tests (`tests/layout/`, which already load
`/backup-plans/plan-1`, whose plan name is 73 characters long, and `/backup-plans/new`) must pass: no horizontal
scroll, footer buttons never clipped at the 1024×640 minimum window.

## 2. Tree clicks

`boxClick` today returns `removeSource | addSource | dialog`. After this change `dialog` no longer exists as
a left-click result.

**Left-click, unchanged:** a source row removes the source; a row outside every source adds it — and still
opens the confirmation dialog whenever that would swallow existing sources **or needs extra rules** (as
`onBox` does today: `removedSources.length > 0 || addedRules.length > 0`; the extra-rules case arises on
restic < 0.19), so a tree click never adds rules unconfirmed; a row from a path-list file shows the "edit
the file" message.

**Left-click on an *included* item P inside a source → exclude P**
- P included normally: append P's **exact rule**.
- P is included only because of a lone re-include line `!P` (a tree edit re-including something a broad
  rule excludes, or the explicit "include everything inside it" option): **delete the `!P` line(s)**, newest
  first, until P is excluded, restoring the earlier state (the undo of the include rows below). A `!P` that
  is carve-out scaffolding — directly followed by `P/*`, or with `!P/…` lines below it — is **never**
  deleted by a click on P itself (that would strand the carve-out as dead lines): P is then only partly
  included, and the click appends P's exact rule, so the next click deletes that exact line and the list is
  back as it was. (my call)
- P is the leaf of a carve-out out of an excluded folder: **undo the carve-out** — delete its leaf `!P`, and also the
  scaffolding (`!A` / `A/*` per level) that no other carve-out in that folder still relies on, so the list
  returns to how it was and no empty `X/` appears (F4). If another carve-out shares a level, that level stays.
- Falls back to appending the exact rule whenever deleting would not leave P excluded.

**Left-click on an *excluded* item P inside a source → include P**

| P is excluded by… | Click does |
|---|---|
| its own exact rule only | delete that line (newest first until P is included; lines the click did not add are not touched otherwise) |
| its exact rule *and* a broad rule | delete the exact line(s) (they do nothing) **and** add `!P` |
| a broad rule only (`*.tmp`, a name, an extension, `**` patterns) | add `!P`; the broad rule is never touched |
| being inside an excluded folder X (by exact or broad rule) | carve out: minimal chain from the highest still-excluded node down to P — `!X`, `X/*`, then for each level `!child`, `child/*`, ending `!P`. A file directly in X takes 3 rules; each extra level adds 2; a sibling of an earlier carve-out takes 1 (F3). |
| a marker file, an exclude-file line, or a case-insensitive rule | refused with the reason (as today) |

*Exact rule* = a non-negated, case-sensitive line in the plan's **own** pattern list (never an exclude file's)
whose text equals P's path with glob characters escaped for the platform (`PathStyle::escape_glob`; Windows
writes `[` as `[[]`). A user-typed line with the same text counts.

A folder excluded **only** by a broad rule gets `!P`, which by F1 also brings back items inside it that other
rules exclude. That is accepted (agreed with the user); the Undo line says so: "also brought back N items
other rules exclude" (N from the same bounded scan as the live match count; "at least N" when capped;
omitted when 0). The same disclosure applies to every edit that re-includes a *folder* with `!` rules,
because F1 applies equally: the "exact rule and a broad rule" row, a carve-out whose leaf P is a folder, and
the explicit "include everything inside it" menu items. (my call: same principle, no behaviour change)

**Right-click** on a row opens `ContextMenu` (the app suppresses the browser menu globally in `main.tsx`;
the row handler must `preventDefault` and open ours). Also opens from the keyboard (menu key / Shift+F10)
on the focused box button, positioned at the button. A hint under the tree: "Click to include or exclude ·
Right-click for more options". Items:

| Row state | Menu |
|---|---|
| included item inside a source | **Exclude** · **Exclude by pattern…** (today's dialog, exclude mode, scopes unchanged) |
| partly included folder (something inside is excluded) | the two above + **Include everything inside it** (`!P`, F1) |
| excluded item | **Include** (same as left-click) |
| excluded folder with an exact rule | + **Include, plus everything inside it that other rules exclude** (appends `!P` even though the plain click would delete the exact line; the exact line stays) |
| excluded folder with only a broad rule | just **Include** (it already does the above) |
| excluded file inside an excluded folder | just **Include** (carve-out). No "include the whole folder" anywhere. |
| source / outside every source | the same single action as the left-click, so right-click is never dead |

`PlanTreeEditDialog` keeps the exclude-pattern mode (reached from the menu). Its include and remove-source
modes are reached only for the add-source confirmation above (swallowed sources or extra rules); the
`scope: "folder"` include variant (whole excluded ancestor) becomes unreachable and is removed from UI and
backend.

**Keyboard.** The pattern dialog is reachable only through the menu, so the shared `ContextMenu` becomes
keyboard-operable, backward-compatibly for its other users (DiffPage, …): focus moves to the first item on
open, Up/Down/Home/End move, Enter activates, Escape closes and returns focus to the trigger. (my call)

## 3. Rule bookkeeping (backend, `plan_edit.rs`)

Every tree edit is computed and verified by the engine before it is returned, as today (`apply_edit`,
`EditOutcome`). New or changed behaviour:

- **Edit kinds.** Exclude-exact, exclude-by-pattern (existing), include (plain), include-everything
  (`!P`). Deleting lines is new: `EditOutcome` carries the full new `excludes` as today; it may add
  `removed_rules` for the summary line.
- **Undo is structural, not historical.** The in-memory undo stack in `FilesTab` keeps working, but click-click
  must also work across sessions, so the engine recognises what it wrote: the exact line, `!P`, and the
  carve-out chain for P (the `carve_out_chain` shape) in the plan's own list.
- **Scope of deletions.** A tree click may delete only: P's exact lines, a `!P` line, carve-out scaffolding
  as above, and tree-written comments (§4). It never deletes or rewrites any other line.
- **Collateral check (new).** After computing an edit, the engine compares verdicts before/after over a
  bounded scan of the plan's sources (same budget idea as `tree_count_matches`). The only nodes allowed to
  flip are P, nodes inside P, and folders on the path to P. If something else flips, the edit is refused
  with a reason (never silently applied). A capped scan still allows the edit, verified on what was scanned.
- Errors keep the current style: a sentence naming the reason; nothing is returned that the engine does not
  confirm.

## 4. Comments in exclude rules

Today the backend already ignores `#` and blank lines everywhere that matters (`build_exclude_args`,
`plan_tree::build_rules`, the plans list counts), and `excludes` is stored as plain strings. Comments are
lost only in the editor: `savedExcludes`/`expertPatterns` strip them on save, and `ExcludeCard.switchMode`
drops them (and de-duplicates with a `Set`) when leaving Expert.

- **Persist them.** Saving keeps comment lines (trimmed; blank lines still dropped). Leaving Expert keeps every
  line in order, including comments and duplicates (order and repeats are meaningful: `X`, `!X`, `X`).
- **Display.** Expert: unchanged text area. Simple: comment lines show as muted rows (italic, no pattern
  styling) with the same × as other rows; the "Add" box accepts `# note` as a comment; duplicate comments
  are allowed. Rule counts anywhere already skip `#` lines — keep it that way.
- **Written by the tree.** Every edit that adds rules puts one comment line directly above them, prefix
  `# Tree: `, one line, control characters in paths replaced. Formats (wording is the builder's, the shape
  is not): `# Tree: excluded <path>` / `# Tree: included <path>` followed by an optional ` — note` that says
  *why* the rules look as they do ("a broader rule excludes it; this re-includes only this item", "reopen
  <X>, exclude what is inside, bring the file back"); pattern-dialog edits use the scope's own label.
  Source add/remove adds no rules and no comment.
- **Removed with their rules, only then.** When an edit deletes the rules a tree comment introduced, it
  deletes that comment too — only if the comment is directly above the first deleted line and starts with
  `# Tree: excluded|included <P>` for the same P (matched against the known path, no general parser).
  Comments the user wrote are never deleted by a click. An orphaned tree comment is harmless.
- **Out of scope:** the "Use with restic CLI" export does not carry comments (it renders parsed rules);
  restic exclude *files* support `#`, so adding them later is easy but not part of this.

## 5. Verification

Pure rules get unit tests; the engine gets its own and the restic differential harness
(`restic_confirms_every_edit_the_engine_offers`, `#[ignore]`, run with
`cargo test restic_confirms -- --ignored --nocapture`; CI's `restic-compat` runs it on 0.17.3 and 0.19.1)
is extended:

1. Clicked item flips as asked (existing).
2. Engine agrees with restic on every node after the edit (existing).
3. **No collateral:** nodes outside P's subtree and the path to P keep their restic result from *before* the
   edit (needs a "before" restic run — the current test only checks after).
4. **Sequences:** random runs of 2–6 clicks per case, including the same item twice. Click then click again
   returns the **identical rule list** (text, comments included) for lines the tree itself added: the
   second click deletes exactly what the first appended, comment and all. For a *deleted hand-written
   line* only an **equivalent included set** is guaranteed — its position (and any comment it never had)
   cannot be restored; the documented case is "exact line and broad rule both excluded P", where the
   second click returns an equivalent list without the dead exact line.
   The generator must also cover: "include everything inside it" then click (back to the start when the
   folder had an exact rule; on a partly included folder the `!P` is not what includes it, so the click
   appends the exact rule after it and the next click deletes that rule), `!P` added twice, and a click on
   a folder whose `!P` is carve-out scaffolding (appends the exact rule; the next click deletes it; the
   list is identical to the start).
5. Broad rules, exclude-file lines and user comments are never modified by a tree click.
6. Windows spellings (`PathStyle::Windows`) for exact-line matching and chains, extending the existing
   `windows_*` unit tests.

Frontend (Vitest): `boxClick` replaced by the new left-click/menu decision (pure, tested); `planForm`
(Settings sections, `FormIssue` mapping, comment-keeping save/load round trip, URL compat mapping,
dots); `ExcludeCard` helpers for keep-everything mode switching. Run `npm run typecheck`, `lint:all`,
`test:all` (includes `test:layout` when Chrome is available) before each commit.

## 6. Docs to update (part of done)

`docs/backup-plan-tree.md` ("Clicking a box", "Editing from the tree", status/roadmap, the verified
facts above), `docs/frontend.md` (`BackupPlanEditPage` structure paragraph, tabs, footer, jump list, menu,
comments in `ExcludeCard`), `docs/decisions.md` (new settled decisions: re-include never touches broad
rules; `!P` semantics and the accepted broad-folder limit; no whole-folder option; tree comments),
`CLAUDE.md` (project-structure lines for `plan/`, `BackupPlanEditPage`, `PlanTreePreview`; one
"Settled decisions" bullet; the Testing line if test files change).

## 7. Delivery

Work on `feat/plan-editor-ux`; commit in reviewable steps, each passing the checks above (suggested order:
engine + tests → tree clicks/menu → comments in the editor → layout → docs; the builder may reorder).
Do not push, open PRs, bump versions, or touch `package.json`/`Cargo.toml` versions.

## Left to the builder

Component/file names and splits; scroll-spy mechanics; exact comment and Undo-line wording; the scaffolding
cleanup and collateral-scan algorithms (the invariants in §3 and §5 are the contract); how the footer
message handles very long errors; whether `PlanTreeEditDialog` is split now that two of its modes are rare.
