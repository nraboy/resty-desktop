# Fix cache-warmer audit findings (F1–F4)

## Context

A prior audit of the new snapshot cache-warming feature (`src-tauri/src/cache_warmer.rs`, untracked WIP) found four issues. Two are confirmed user-facing bugs; two are plausible correctness/scaling gaps. This change fixes all four and adds Rust unit tests for the two pure-DB fixes.

- **F1 (confirmed) — Modal/menu stuck when the warmer is already indexing.** `index_snapshot` early-returns `Ok(())` with **no event** when a snapshot is already `complete`/`in_progress` (`browse.rs:290-295`). The SnapshotsPage opens a modal optimistically and only closes it via the `index:done` event (`SnapshotsPage.tsx:97-114`). Worse, it sets `indexStatus[snap.id] = "in_progress"` before the call (`:1129`) and never reconciles it — so the "Index Snapshot" row stays **disabled forever**, not just the modal spin.
- **F2 (confirmed) — Warmer completions never reach the frontend.** `cache_warmer.rs::index_next` runs `run_full_index` for background snapshots but never emits `index:done` (only the manual `index_snapshot` path emits it, `browse.rs:328`). So the menu stays enabled after the warmer finishes.
- **F3 (plausible) — `evict()` wipes other repos' index status.** `browse_cache_status` has composite PK `(repo_id, snapshot_id)` (`cache.rs:255-260`), but `evict` deletes by `snapshot_id` alone (`cache.rs:723-727`). After `restic copy`, the same `snapshot_id` lives in two repos; deleting from repo A nukes repo B's status row.
- **F4 (plausible) — DB mutex held across the whole repo loop.** `get_next_unindexed_snapshot` locks `self.conn` and loops a prepared statement over every eligible repo (`cache.rs:814-834`), blocking all other `AppDb` methods for the full loop.

F1 and F2 share one root fix (a consistent `index:done` contract), so they land together. F3 and F4 are independent.

## Fix 1 + Fix 2 — consistent `index:done` contract

**Backend — `src-tauri/src/commands/browse.rs` (`index_snapshot`, lines 279-335):**
- Line 286: return type `Result<(), String>` → `Result<bool, String>`.
- Line 294 (early-return for `complete`/`in_progress`): `return Ok(());` → `return Ok(false);`.
- Line 334 (after spawning the task): `Ok(())` → `Ok(true)`. (The existing `emit` at 328 is unchanged and still fires on the manual path.)
- Semantics: `true` = "I spawned a task that will emit `index:done`"; `false` = "already handled, don't wait."

**Backend — `src-tauri/src/cache_warmer.rs` (`index_next`, lines 113-127):**
- Line 6: `use tauri::Manager;` → `use tauri::{Emitter, Manager};` (`emit` lives on `Emitter`).
- Insert **before** the `spawn_blocking(move || …)` (between lines 113 and 116):
  `let emit_repo_id = repo_id.clone();` and `let emit_snapshot_id = snapshot_id.clone();`
  (The `move ||` closure consumes `repo_id`, `snapshot_id`, `app2`, `repo`, `restic_path`. The function param `app: &AppHandle` is **not** moved — only the `app2` clone is — so `app` is still usable after the `.await`. `AppHandle: Sync` ⇒ `&AppHandle: Send`, safe across the await.)
- After the `.await … .unwrap_or(false)` (after line 125), emit:
  ```rust
  let _ = app.emit("index:done", serde_json::json!({
      "snapshotId": emit_snapshot_id,
      "repoId": emit_repo_id,
      "success": ok,
  }));
  ```
  Payload shape must match `browse.rs:328-331` exactly so the existing listener works unchanged. (Cross-repo events are harmless: `browse_cache_files` is keyed by `snapshot_id` only, so a warmed snapshot genuinely benefits any repo sharing that id.)

**Frontend — `src/lib/invoke.ts` (lines 193-194):**
- `indexSnapshot` return type `Promise<void>` → `Promise<boolean>`.

**Frontend — `src/pages/SnapshotsPage.tsx` (onClick, lines 1123-1131):** rewrite to `async`. Close the menu first, await the result, and **only** set modal/optimistic state when `started === true`:
```tsx
onClick: async () => {
  const snap = contextMenu.snap;
  setContextMenu(null);
  let started = false;
  try { started = await indexSnapshot(repoId!, snap.id); }
  catch { started = false; }
  if (!started) {
    // already complete or already in progress (e.g. warmer mid-flight):
    // reconcile the row's true state instead of opening a modal that would hang.
    getSnapshotIndexStatus(repoId!).then(setIndexStatus).catch(() => {});
    return;
  }
  setIndexingTarget(snap);
  setIndexingDone(false);
  setIndexingSuccess(true);
  setIndexStatus((prev) => ({ ...prev, [snap.id]: "in_progress" }));
},
```
This removes the optimistic state set that previously ran *before* the invoke.

**Cleanup (optional, defense-in-depth) — `src/pages/SnapshotsPage.tsx:1200`:** relax the `onClose` gating to match the in-modal Close button (`:1238`), so Esc/overlay/X always dismiss:
`onClose={() => { if (indexingDone) { setIndexingTarget(null); setIndexingDone(false); } }}` → `onClose={() => { setIndexingTarget(null); setIndexingDone(false); }}`.

## Fix 3 — scope `evict` to `(repo_id, snapshot_id)`

**`src-tauri/src/commands/cache.rs` (`evict`, lines 716-729):**
- Line 716 signature: `evict(&self, snapshot_id: &str)` → `evict(&self, repo_id: &str, snapshot_id: &str)`.
- Leave the `browse_cache_files` DELETE keyed by `snapshot_id` only (the table has no `repo_id` column; content is shared across repos — `cache.rs:241-252`).
- Change the `browse_cache_status` DELETE to `WHERE repo_id = ?1 AND snapshot_id = ?2` with `params![repo_id, snapshot_id]`.

**`src-tauri/src/commands/snapshot.rs:77` (sole caller, inside `delete_snapshot`, `repo_id` in scope):**
- `db.evict(&snapshot_id)` → `db.evict(&repo_id, &snapshot_id)`.

## Fix 4 — single-query `get_next_unindexed_snapshot`

**`src-tauri/src/commands/cache.rs` (lines 807-836):** replace the per-repo loop with one query using `IN (?, …)`:
- Keep the `is_empty()` guard (line 811) so we never emit `IN ()`.
- Build `let placeholders = eligible_repo_ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");`.
- `format!` the SQL: same JOIN, but `WHERE sc.repo_id IN ({placeholders}) AND (bcs.status IS NULL OR bcs.status = 'pending') LIMIT 1`.
- Prepare once, `query_row(rusqlite::params_from_iter(eligible_repo_ids.iter()), …)`. (`params_from_iter` with `&String` items is already proven in this file at `cache.rs:1286`; no import change needed — use the fully-qualified path.)
- Map `rusqlite::Error::QueryReturnedNoRows` → `Ok(None)`.
- The mutex is now held for one query then released. Accepted trade-off: repo ordering is no longer the strict `eligible_repo_ids` order — irrelevant for a warmer that needs *any* unindexed snapshot, and a snapshot drops out of the result set once indexed so the loop always progresses.

## Tests — add a `#[cfg(test)]` module in `cache.rs`

Harness (matches production init at `lib.rs:198-200`):
```rust
fn test_db() -> AppDb {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    AppDb::init_schema(&conn).unwrap();   // WAL pragma is a harmless no-op in-memory
    AppDb::new(conn)
}
```
- **F3 test:** insert two `browse_cache_status` rows `(repoA, snap)` and `(repoB, snap)` (both `complete`); call `evict("repoA", "snap")`; assert repoA's row is gone and repoB's survives. (Pin via `get_browse_status` on each repo, or a direct `SELECT COUNT`.)
- **F4 test:** seed `snapshots_cache` for two repos and `browse_cache_status` with one `pending`/one `complete`; assert `get_next_unindexed_snapshot(&[repoA, repoB])` returns `Some` for the unindexed pair and `Ok(None)` once both are `complete`; assert `Ok(None)` on empty input.

F1/F2 are event-driven and covered by the manual E2E steps below, not unit tests.

## Verification

**Compile/tests (run first, must be clean):**
- `cargo check --manifest-path src-tauri/Cargo.toml` — confirms move/borrow for the emit, `Emitter` import, `params_from_iter`, and the `bool` return.
- `npm run test:rust` — existing tests pass + the two new tests pass.
- `npm run test:vite` — no SnapshotsPage/invoke tests to break.

**Manual E2E (`npm run tauri dev`):**
- **F1:** on a snapshot the warmer already finished, "Index Snapshot" → no modal, row stays disabled; on one mid-flight → no modal, status reconciles to `in_progress` then flips to `complete` when the warmer emits; on an unindexed one → modal opens, spinner, closes on `index:done`.
- **F2:** clear `browse_cache_status` (or wait) and watch rows flip enabled→disabled as the warmer indexes them.
- **F3:** `restic copy` a snapshot A→B (same id in both), index both, delete it in A with prune; verify `sqlite3 app_data.db "SELECT * FROM browse_cache_status WHERE snapshot_id='<id>';"` shows only repo B's row surviving. (B's `browse_cache_files` will be gone and self-heal on next browse — acceptable, see Fix 3 note.)
- **F4:** with several repos, confirm the warmer still progresses through all unindexed snapshots (same observable behavior, mutex no longer held across the loop).

## Out of scope (noted by the audit as non-bugs)

Auto-cleanup of `browse_cache_files`/`browse_cache_status` on repo delete; unbounded `browse_cache_files` growth; `is_fully_indexed` ignoring `repo_id`; TOCTOU on concurrent manual+warmer indexing; warmer backoff for a perpetually-failing snapshot (a failed index resets to `pending` and is retried next 60s sweep, stalling later snapshots in that repo). These can be a follow-up.

## Files touched

- `src-tauri/src/commands/browse.rs` — `index_snapshot` return type + early-return value.
- `src-tauri/src/cache_warmer.rs` — `Emitter` import, clone-before-closure, post-await `emit`.
- `src-tauri/src/commands/cache.rs` — `evict` signature/DELETE; `get_next_unindexed_snapshot` rewrite; new `#[cfg(test)]` module.
- `src-tauri/src/commands/snapshot.rs` — `evict` call site.
- `src/lib/invoke.ts` — `indexSnapshot` return type.
- `src/pages/SnapshotsPage.tsx` — async onClick + (optional) `onClose` relaxation.
