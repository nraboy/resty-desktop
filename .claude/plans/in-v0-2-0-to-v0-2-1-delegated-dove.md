# Fix: launch hang on upgrade (v0.2.0 → current) from migration VACUUM

## Context

A user reported the app became unresponsive when launching v0.2.1 for the first
time after upgrading from v0.2.0. The v0.2.0 → v0.2.1 release introduced a v1→v2
SQLite schema migration (commit `61902b9` — "Shrink browse-cache storage: intern
snapshot_id, drop redundant columns") that drops the browse-cache tables.

The cache drop itself is fast (O(1) metadata ops), but the migration *also* runs a
`VACUUM` + `PRAGMA wal_checkpoint(TRUNCATE)` to reclaim the freed disk space.
`VACUUM` is O(file size) and holds an exclusive writer lock for its entire
duration. On an install that had indexed many snapshots, `browse_cache_files` can
be hundreds of MB (the `parent_path` duplication is the single largest size
contributor), so this single operation blocks for seconds to minutes.

The blocking runs **synchronously on the main thread** inside Tauri's `.setup()`
closure — the webview window is not created until `.setup()` returns, so the user
sees a blank/non-responsive window until the VACUUM finishes. It fires exactly
once, gated by `user_version < 2`, matching the "first launch of v0.2.1" report.

**Still live:** the migration code at HEAD is byte-for-byte identical to v0.2.1's,
so any user still on v0.2.0 (or earlier) will hit this when they next upgrade to
current. This fix prevents that.

**Why dropping the VACUUM is safe (no cache doubling):** `DROP TABLE` deletes the
rows and moves the pages to SQLite's **freelist**; the data is gone, the tables are
recreated empty. When the cache rebuilds via re-indexing, SQLite allocates new
pages from the freelist *before* growing the file, so the rebuilt cache **refills
the freed space in place** rather than appending a second copy. Worst case the
file holds steady at its old high-water mark (some of it free pages); it never
becomes old+new. The file won't *shrink* without a VACUUM, but that is harmless
(freelist pages are tracked efficiently and reused) — a disk-usage/cosmetic
concern only.

## Root cause

`src-tauri/src/commands/cache.rs`, `AppDb::init_schema`, the `if version < 2`
block (lines 266–283). Lines 276–282:

```rust
// DROP TABLE only frees pages into SQLite's internal freelist —
// it doesn't shrink the file on disk. ...
let _ = conn.execute_batch("VACUUM;");
let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
```

Called from `src-tauri/src/lib.rs` line 200 inside the synchronous `setup(|app| …)`
closure, before `app.manage(app_db)` and before the window is shown.

## The fix

In `AppDb::init_schema` (`src-tauri/src/commands/cache.rs`), inside the
`if version < 2` block:

- **Keep** the `DROP TABLE IF EXISTS browse_cache_files / browse_cache_status` +
  `PRAGMA user_version = 2` batch (lines 271–275) — those are the actual migration
  and are O(1).
- **Delete** lines 276–282 (the "DROP TABLE only frees pages…" comment, the
  `VACUUM`, and the `wal_checkpoint(TRUNCATE)`).
- Replace with a short comment: the freed pages are intentionally left on SQLite's
  freelist (reused in place as the disposable cache rebuilds via re-indexing — no
  doubling); we deliberately do not VACUUM here because doing so on the main thread
  blocks window creation for an O(file-size) rewrite on upgrade. Users who want to
  shrink the file can use "Clear All Cache", which already does its own VACUUM via
  `clear_cache` (`cache.rs:1606`).

No change to `lib.rs` — `init_schema` without the VACUUM is fast (DROPs + `CREATE
TABLE IF NOT EXISTS` no-ops), so it stays fine on the setup thread. No change to
`clear_cache` / `checkpoint_and_size` — those are user-triggered, not on the launch
path, and remain as-is.

## Verification

1. **New regression test** in the `#[cfg(test)]` module of
   `src-tauri/src/commands/cache.rs`. Construct a raw `Connection` in a v1 state:
   set `PRAGMA user_version = 1`, create v1-shaped `browse_cache_files` (with the
   old `snapshot_id TEXT`/`name`/per-row `cached_at` columns) and
   `browse_cache_status`, and insert enough rows to span many pages (a few hundred
   to a couple thousand). Then call `AppDb::init_schema(&conn)` and assert:
   - `PRAGMA user_version` is now `2`.
   - The v2 `browse_cache_files` table exists and has the interned `snap` column
     (via `PRAGMA table_info`).
   - **`PRAGMA freelist_count > 0`** — the dropped-table pages must remain on the
     freelist, proving a `VACUUM` did **not** run (a VACUUM would have reclaimed
     them to ~0). This is the regression guard against re-introducing the bug.
2. **Existing tests still pass:** `npm run test:rust` (and `npm run test:vite` for
   completeness — no frontend change, but cheap).
3. **Manual smoke (optional):** with `npm run tauri dev`, point the app at an
   `app_data.db` hand-crafted to `user_version = 1` with a large
   `browse_cache_files`, confirm the window appears promptly on first launch (no
   multi-second blank window), and that `user_version` is bumped to 2 afterward.

## Out of scope (per decision)

- **No "Compact database" button** — deferred. A non-destructive reclaim path
  (lock + VACUUM + `checkpoint_and_size`, no deletes) is a reasonable future
  addition but is not needed for this fix; the leftover free space is harmless and
  reused in place by re-indexing. The only reclaim path today remains "Clear All
  Cache" (destructive).
- **No change to the post-upgrade auto re-index.** Re-indexing is already strictly
  sequential (one snapshot at a time, gated by `IndexHandle::gate` +
  `manual_active` + the `running` AtomicBool), and writes are chunked at 500 rows
  with one short mutex hold per chunk (cache.rs:1041–1042), so it does not starve
  UI commands. Not a contributor to the launch hang.
