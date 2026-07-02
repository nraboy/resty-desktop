# Shrink the file-index cache ("Quick wins")

## Context

Indexing a snapshot writes one row per file into `browse_cache_files`, adding
20–70 MB per snapshot. The rows are heavier than they need to be:

- **`snapshot_id` is a 64-char hex string stored 3–4× per file** — once in the
  row, again in the PK index `(snapshot_id, path)`, again in the secondary index
  `(snapshot_id, parent_path)`. This is the single largest cost (~190 bytes/file).
- **`name` is redundant** — it's just the last path segment, and its `LIKE`
  search clause is fully subsumed by the existing `path LIKE` clause.
- **`cached_at` is redundant per-row** — identical for every file of a snapshot.

This "Quick wins" pass removes those three costs, cutting per-snapshot size
~40–50% with a small, low-risk change. It does **not** attempt cross-snapshot
path dedup (paths are still re-stored per snapshot) — that's a possible future
follow-up, noted at the bottom.

The browse cache is fully rebuildable from `restic ls --json`. This is a
pre-release and the DB is purged on upgrade, so we **do not** write a data
migration — we bump `PRAGMA user_version`, drop + recreate the cache tables, and
let re-indexing repopulate. No user data (repos, passwords, plans, schedules,
history) lives in these tables; they are separate and untouched.

## Design principle: intern internally, keep public signatures on hex IDs

All `AppDb` methods keep taking `snapshot_id: &str` (the hex id) at their
boundary, so callers in `browse.rs` and `cache_warmer.rs` **do not change**. The
integer interning is entirely internal to `cache.rs`: each write looks up (or
creates) the integer key; each read/delete maps hex→int (or int→hex) via a small
join or subquery. This keeps the change contained to one file.

`browse_cache_status` stays keyed by `(repo_id, snapshot_id)` **hex** — it's one
tiny row per snapshot, keyed by repo, so there's nothing to gain from interning
it. `cached_at` moves *here* (one value per snapshot).

## Critical file

`src-tauri/src/commands/cache.rs` — all schema and query changes. One small
helper is added to `browse.rs` only if we prefer to recompute `name` there;
otherwise a `name_of` helper lives next to `parent_path_of` in `cache.rs`.

---

## Changes in `cache.rs`

### 1. Schema (`init_schema`, ~L204–312)

Add a `version < 2` migration block **before** the `CREATE TABLE IF NOT EXISTS`
batch (mirroring the existing `version < 1` block at L205–211):

```sql
DROP TABLE IF EXISTS browse_cache_files;
DROP TABLE IF EXISTS browse_cache_status;
PRAGMA user_version = 2;
```

New table definitions (replace L243–262):

```sql
CREATE TABLE IF NOT EXISTS indexed_snapshots (
    id           INTEGER PRIMARY KEY,
    snapshot_id  TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS browse_cache_files (
    snap         INTEGER NOT NULL,      -- indexed_snapshots.id
    path         TEXT NOT NULL,
    parent_path  TEXT NOT NULL,
    entry_type   TEXT NOT NULL,
    size         INTEGER,
    mtime        TEXT,
    mode         INTEGER,
    PRIMARY KEY (snap, path)
);
CREATE INDEX IF NOT EXISTS idx_browse_files
    ON browse_cache_files (snap, parent_path);
CREATE TABLE IF NOT EXISTS browse_cache_status (
    repo_id      TEXT NOT NULL,
    snapshot_id  TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'pending',
    cached_at    INTEGER,               -- NEW: one timestamp per snapshot
    PRIMARY KEY (repo_id, snapshot_id)
);
```

Dropped columns vs. today: `browse_cache_files` loses `name` and `cached_at`;
`snapshot_id TEXT` becomes `snap INTEGER`.

Optional (low effort, fresh-DB only): issue `PRAGMA page_size = 8192;` before the
first `CREATE TABLE` runs on a brand-new DB, so these many-small-row tables pack
a bit tighter. Existing/purged DBs pick it up on next create or `VACUUM`.

### 2. Interning helpers (new, private)

- `fn intern_snapshot(tx: &Transaction, snapshot_id: &str) -> Result<i64>`:
  `INSERT OR IGNORE INTO indexed_snapshots(snapshot_id) VALUES (?1)` then
  `SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1`. Used by writers.
- `fn snap_id_of(conn, snapshot_id) -> Result<Option<i64>>`: `SELECT id ...`;
  returns `None` when the snapshot was never indexed. Used by readers/deleters.
- `fn name_of(path: &str) -> String` next to `parent_path_of` (~L1639): last
  segment after the final `/` (trailing slash trimmed, same as `parent_path_of`).
  Used to rebuild `FileEntry.name` / `RepoFileHit.name` from `path` on read.

### 3. Writers

- **`insert_browse_files` (~L888)**: at the top of each chunk's transaction, call
  `intern_snapshot(&tx, snapshot_id)` once to get `snap`. Change the INSERT to
  the new column set `(snap, path, parent_path, entry_type, size, mtime, mode)`
  — drop `name` and `cached_at`.
- **`set` (~L712)** (per-directory lazy cache during browse): same treatment —
  intern once, DELETE `WHERE snap = ?1 AND parent_path = ?2`, INSERT without
  `name`/`cached_at`. Wrap in a transaction so intern + delete + inserts are
  atomic (today it's loose `execute` calls).
- **`set_browse_status` (~L785)**: also write `cached_at` (= `timestamp()`), so
  each snapshot's index time is recorded once here instead of per file row.

### 4. Readers

- **`get` (~L675, directory listing)**: resolve `snap_id_of(snapshot_id)`; if
  `None`, treat as cache miss (return `Ok(None)` unless `fully_indexed`). Query
  becomes `SELECT path, entry_type, size, mtime, mode FROM browse_cache_files
  WHERE snap = ?1 AND parent_path = ?2`, and build each `FileEntry` with
  `name: name_of(&path)`. (`is_fully_indexed` is unchanged — it reads
  `browse_cache_status` by hex id.)
- **`search_browse_files` (~L803)**: map hex→`snap` first; query
  `WHERE snap = ?1 AND path LIKE ?2 ESCAPE '\\'` — **drop the `name LIKE` clause**
  (redundant). Rebuild `name` via `name_of(&path)`.
- **`search_repo_files` (~L844)**: join through `indexed_snapshots` to translate
  `snap`↔hex. New shape:

  ```sql
  SELECT bcf.path, bcf.entry_type, bcf.size, bcf.mtime, bcf.mode,
         isn.snapshot_id, sc.short_id, MAX(sc.time)
  FROM browse_cache_files bcf
  JOIN indexed_snapshots isn ON isn.id = bcf.snap
  JOIN snapshots_cache sc    ON sc.snapshot_id = isn.snapshot_id AND sc.repo_id = ?1
  JOIN browse_cache_status bcs ON bcs.snapshot_id = isn.snapshot_id
                               AND bcs.repo_id = ?1 AND bcs.status = 'complete'
  WHERE bcf.path LIKE ?2 ESCAPE '\\'
  GROUP BY bcf.path
  ORDER BY bcf.path
  LIMIT ?3
  ```

  Populate `RepoFileHit { name: name_of(&path), snapshot_id: isn.snapshot_id, .. }`.

### 5. Deleters / maintenance (map hex→int and clean up `indexed_snapshots`)

- **`evict` (~L748)**: `DELETE FROM browse_cache_files WHERE snap =
  (SELECT id FROM indexed_snapshots WHERE snapshot_id = ?1)`, then
  `DELETE FROM indexed_snapshots WHERE snapshot_id = ?1`, then delete the status
  row (unchanged). (A no-op `snap` lookup is harmless — deletes just match nothing.)
- **`remove_repo` (~L420)**: after deleting status rows, delete files via
  `WHERE snap IN (SELECT id FROM indexed_snapshots WHERE snapshot_id IN
  (SELECT snapshot_id FROM snapshots_cache WHERE repo_id = ?1))`, then delete the
  matching `indexed_snapshots` rows with the same subselect.
- **`clean_cache` (~L1490)**: rewrite the two browse deletes to key off `snap`,
  and add a delete of orphaned `indexed_snapshots` rows
  (`WHERE snapshot_id NOT IN (SELECT snapshot_id FROM snapshots_cache)`) plus
  files whose `snap` has no `indexed_snapshots` parent. Order the deletes so
  files go before their `indexed_snapshots` row.
- **`clear_cache` (~L1468)** and **`reset_all` (~L1535)**: add
  `DELETE FROM indexed_snapshots;` to the batch.

### 6. Tests (`#[cfg(test)]` in `cache.rs`)

- Update the existing migration regression test to also assert
  `user_version == 2`, that `indexed_snapshots`/`browse_cache_files`/
  `browse_cache_status` exist with the new columns, and that the old
  `browse_cache_files.name`/`cached_at` columns are gone.
- Add a round-trip test: `insert_browse_files` two snapshots that share a
  file → `get` lists the right children with correct `name`; `search_browse_files`
  and `search_repo_files` return the file (repo search deduped to the newest
  snapshot with correct `snapshot_id`); `evict` removes one snapshot's rows and
  its `indexed_snapshots` entry while the other survives; `clean_cache` drops
  rows for a deleted repo and leaves no orphan `indexed_snapshots`.

## Out of scope (possible future follow-up)

Cross-snapshot **path dictionary** dedup (intern each unique path once, shared
across snapshots, so 2nd+ snapshots store only integer + metadata rows). That
removes the *compounding* growth but adds path garbage-collection complexity.
Not needed now; the schema above leaves room to add it later behind another
`user_version` bump.

## Verification

1. `npm run test:rust` — the updated/new cache unit tests must pass (SQL column
   changes fail at runtime, so these tests are the safety net).
2. `npm run test:all`.
3. `npm run tauri dev`: add a repo, index a snapshot, and confirm on the Settings
   page that `get_db_size` reports a noticeably smaller DB than before for the
   same snapshot (compare against a pre-change build, or against a second larger
   snapshot to sanity-check scaling).
4. Exercise every cache reader/writer to confirm no regression:
   - Browse into nested directories (BrowsePage) — names/sizes/types render.
   - Single-snapshot search (SearchPage) — results match by name and by path.
   - Repo-wide search (RepoSearchPage) — results show the correct snapshot-id
     badge and open the right BrowsePage.
   - "Remove Index" on a snapshot (evict) — it disappears from the status map and
     re-shows "Index Snapshot".
   - Settings → "Clean Orphaned" and "Clear All Cache" — DB size updates, no
     errors, and no leftover `indexed_snapshots` rows (spot-check via a test).
   - Delete a repo — its cached files/status/`indexed_snapshots` are gone.
