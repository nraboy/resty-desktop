# Polish Pass Round 2: Destructive-Action Confirmations + Robustness Cleanup

## Context

Following the first polish round (accessibility, DB pragmas, test coverage — already merged into
the working tree), a fresh UX exploration surfaced two genuine defects with **real data-loss risk**
plus a set of smaller robustness gaps. The user chose to pursue **destructive-action confirmations**
and **robustness cleanup**. (Keyboard accessibility and friendlier error messages were surfaced but
deferred by the user this round.)

Two items originally on the robustness list were verified as non-issues and dropped:
- SnapshotsPage `page = -1` — already guarded by `if (filtered.length === 0) return;`
  (`SnapshotsPage.tsx:391`).
- `log_backup` single transaction — its insert-then-trim ordering is deliberate (durable insert
  before trim); wrapping adds no real value.

---

## 1. Destructive-action confirmations

### 1a. "Delete Plan" fires with no confirmation — `src/pages/BackupPlanEditPage.tsx`
The `Delete Plan` button (`:597-601`) calls `handleDelete` (`:252`) directly, which immediately
`removeBackupPlan`s and navigates away — inconsistent with every other delete in the app, and a
single misclick destroys the plan.

**Fix:** gate it behind a confirm modal, reusing the **exact existing pattern** from
`src/pages/BackupPlansPage.tsx:547-561` (the list page's "Delete Backup Plan" modal) — same wording
("This only removes the plan definition — existing snapshots are not affected."), a `Cancel` +
danger `Delete` pair, and `loading={deleting}`. Add a `confirmDelete` boolean state; the header
`Delete Plan` button opens the modal; the modal's `Delete` button calls the existing `handleDelete`.
`Modal` is already the shared component (`src/components/Modal.tsx`) — import it if not already.

### 1b. "Prune All Repositories" fires on click — `src/pages/SettingsPage.tsx`
`onClick={() => { setPruneModalOpen(true); handlePruneAll(); }}` (`:602`) opens the modal **and**
starts the irreversible all-repos prune in the same handler; the modal (`:607+`) is only a
progress/result display. The single-repo prune on `RepositoriesPage.tsx:810-822` already confirms
first, so the higher-stakes bulk action has weaker protection.

**Fix:** change the button to only `setPruneModalOpen(true)` (do **not** call `handlePruneAll`).
Add an initial **confirm branch** to the existing prune modal (before the `pruneDone`/`pruneCancelled`/
`pruneError`/progress branches at `SettingsPage.tsx:608+`): show a short warning that prune permanently
removes unreferenced data across all repositories and cannot be undone, with `Cancel` (closes modal)
and a danger `Prune All` button that calls `handlePruneAll`. Track with a `pruneStarted` flag so the
confirm view shows only before the run begins. Keep `closePruneModal` resetting that flag.

*(Out of scope this round, noted for consistency: "Clear All Cache" also fires without confirm —
low severity since the cache is rebuildable.)*

---

## 2. Robustness cleanup

### 2a. Settings toggles swallow persistence failures — `src/pages/SettingsPage.tsx`
The auto-indexing (`:346-350`), remote-auto-refresh (`:376-380`), and tray (`handleTrayToggle`,
`:128-136`) handlers flip local state optimistically then `.catch(() => {})` the persist call, so a
failed save leaves the UI showing a state the backend never stored.

**Fix:** on persist failure, revert the optimistic local state and surface the error via the page's
existing `setError(String(err))` banner (same pattern as `handleSave`, `:121-122`). For the tray
toggle, revert if either `setTrayEnabled` or the `activate/deactivateTray` call fails.

### 2b. Shared `Spinner` component — `src/components/` + spinner call sites
The `animate-spin` SVG is hand-duplicated ~10 times across pages at varying sizes
(`SnapshotsPage.tsx:1251`, `DiffPage.tsx:174`, `BackupPlansPage.tsx:512`, `RepositoriesPage.tsx:507`,
`RepoSearchPage.tsx:239,299`, `SearchPage.tsx:201,234,269`, `BrowsePage.tsx:365`), and some pages use
plain "Loading…" text instead.

**Fix:** add a small `src/components/Spinner.tsx` (accept a size class / `className` prop, default
`text-blue-400`) matching the existing SVG markup, and replace the inline SVGs with it. Keep it
minimal and presentational — no behavior change. This consolidates markup and unifies the loading
look. Match the surrounding components' style (see `Button.tsx`, `EmptyState.tsx`).

### 2c. `NaN` guard on retention/bandwidth numeric parsing — `src/pages/BackupPlanEditPage.tsx:214`
`toNum = (s) => s.trim() === "" ? undefined : parseInt(s, 10)` can store `NaN` for a pasted/transient
non-numeric value. Harden to return `undefined` when `Number.isNaN(n)`.

### 2d. `prepare_cached` on the remaining hot readers — `src-tauri/src/commands/cache.rs`
Round 1 converted `get()`/`search_browse_files()`/`search_repo_files()`. Apply the same
`prepare` → `prepare_cached` change to the other frequently-called readers with static SQL:
- `get_browse_status()` — `cache.rs:928`
- `get_snapshots_vec()` — `cache.rs:1192` (runs on every snapshot-list load)
- `list_backup_history()` — `cache.rs:1364`

Behavior-identical; only caches the compiled statement.

---

## Verification

- **Rust:** `npm run test:rust` (existing suite still green — the `prepare_cached` swaps are
  behavior-neutral and already covered by the cache tests), `npm run lint:rust` (clippy clean).
- **Frontend:** `npm run typecheck`, `npm run lint`, `npm run test:vite`.
- **Manual smoke (`npm run tauri dev`):**
  - Edit a plan → `Delete Plan` now prompts; Cancel keeps it, confirm deletes and navigates.
  - Settings → `Prune All Repositories` now shows a confirm step before any prune runs.
  - Toggle a Settings switch; confirm the spinner/loading looks consistent across pages and that a
    (simulated) persist failure reverts the toggle and shows the error banner.
  - Create/edit a plan with retention/bandwidth fields; confirm values persist and no `NaN` is stored.
- Run `npm run test:all` and `npm run lint:all` before committing.
