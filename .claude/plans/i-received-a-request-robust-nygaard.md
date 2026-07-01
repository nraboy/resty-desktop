# Repo-level cross-snapshot file search

## Context

A user requested the ability to search for files across **all** snapshots of a
repository, not just within a single snapshot. Today, search is scoped to one
snapshot (`SearchPage` at `/snapshots/:repoId/:snapshotId/search`, backed by
`search_snapshot_files`). This plan adds a repo-scoped search initiated from the
repository list that searches every fully-indexed snapshot at once, lists
matching files, and — on click — opens the file in the existing BrowsePage of
the snapshot that contains it.

**Confirmed UX decisions:**
- **Duplicates → "Latest snapshot only":** a path that exists in many snapshots
  is shown once, resolved to the newest snapshot that contains it. No per-file
  snapshot picker.
- **Partial indexing → "Search indexed, offer Index All":** search runs
  immediately against whatever snapshots are already indexed. A banner shows
  "Searching N of M snapshots" with an **Index All** button to fill the gaps.
  Non-blocking.

## Feasibility note

`browse_cache_files` is keyed by `snapshot_id` only (no `repo_id`), but
`snapshots_cache` maps `repo_id → snapshot_id` (with `short_id`/`time`) and
`browse_cache_status` tracks `(repo_id, snapshot_id, status)`. A repo-wide search
is a JOIN across these — **no schema change needed**. Precedent for the join
pattern already exists (`cache.rs:435-439`, `cache.rs:889-897`).

## Backend (`src-tauri`)

### 1. New DB method — `commands/cache.rs`
Add `search_repo_files(&self, repo_id, query, limit) -> Result<Vec<RepoFileHit>>`
next to `search_browse_files` (`cache.rs:803`). Reuse the same LIKE-escaping
(`cache.rs:811`). Dedup by path, picking the newest snapshot via SQLite's
bare-column + `MAX()` guarantee:

```sql
SELECT bcf.name, bcf.path, bcf.entry_type, bcf.size, bcf.mtime, bcf.mode,
       bcf.snapshot_id, sc.short_id, MAX(sc.time)
FROM browse_cache_files bcf
JOIN snapshots_cache sc
  ON bcf.snapshot_id = sc.snapshot_id AND sc.repo_id = ?1
JOIN browse_cache_status bcs
  ON bcs.snapshot_id = bcf.snapshot_id AND bcs.repo_id = ?1 AND bcs.status = 'complete'
WHERE (bcf.name LIKE ?2 ESCAPE '\' OR bcf.path LIKE ?2 ESCAPE '\')
GROUP BY bcf.path
ORDER BY bcf.path
LIMIT ?3
```

Return a small new struct (define in `browse.rs` beside `FileEntry`):
`RepoFileHit { #[serde(flatten)] entry: FileEntry, snapshot_id: String, snapshot_short_id: String }`
(or a flat struct with the same fields) so each hit carries the snapshot to open.

### 2. New command — `commands/browse.rs`
Add `search_repo_files(db, repo_id, query) -> Result<Vec<RepoFileHit>>` mirroring
`search_snapshot_files` (`browse.rs:337`), minus the per-snapshot status gate
(the JOIN already restricts to `complete` snapshots). Trim empty query → `[]`.
Cap at 200 (reuse the existing limit constant/value).

Register in the `invoke_handler!` macro in `src-tauri/src/lib.rs`.

### 3. Index-count helper
Reuse the existing `get_snapshot_index_status(repoId)` command — the frontend
derives "N of M indexed" from that map plus the cached snapshot count. No new
command needed for the banner. "Index All" loops the existing `index_snapshot`
over unindexed snapshot ids (see frontend below).

## Frontend (`src`)

### 4. Typed wrapper — `src/lib/invoke.ts`
Add `searchRepoFiles(repoId, query): Promise<RepoFileHit[]>` beside
`searchSnapshotFiles` (`invoke.ts:202`). Add the `RepoFileHit` type to
`src/lib/types.ts` (FileEntry fields + `snapshotId` + `snapshotShortId`).

### 5. New page — `src/pages/RepoSearchPage.tsx`
Model closely on `SearchPage.tsx` (reuse its `FileIcon`, `browseTarget`,
300ms-debounce, `window.history.replaceState` restore pattern), but repo-scoped:
- Route: `/snapshots/:repoId/search` (3 segments — unambiguous vs. the
  4-segment per-snapshot search route). Register in `src/App.tsx` (`:177`).
- On mount, load `listSnapshots(repoId)` (cache-only, fast) + `getSnapshotIndexStatus(repoId)`
  to build an id→Snapshot map and compute indexed/total counts.
- **Banner** above the input: "Searching N of M snapshots" + **Index All** button
  (hidden when N === M). Index All iterates unindexed ids calling `indexSnapshot`;
  listen for `index:done` to increment the count live and re-run the current query
  (same listener pattern as `SnapshotsPage.tsx:97-118`). Show a subtle spinner
  while indexing is in flight.
- **Results:** flat list keyed by path (already deduped server-side). Each row
  shows path + name (like `SearchPage`) plus a small snapshot short-id chip
  (`snapshotShortId`) so the user sees which snapshot it resolved to.
- **Click → BrowsePage:** persist `{restoredQuery, restoredResults}` into the
  current history entry (`replaceState`), then
  `navigate('/snapshots/${repoId}/${hit.snapshotId}/browse', { state: { snapshot: snapMap[hit.snapshotId], initialPath, initialPathStack, fromSearch: true } })`.
  BrowsePage's `fromSearch` back button already uses `navigate(-1)`, which returns
  here and restores query+results — no BrowsePage change required.
- Empty/`not_indexed`/searching states reuse `SearchPage`'s markup. When **zero**
  snapshots are indexed, show the same "not indexed" empty state but with an
  "Index All Snapshots" CTA instead of per-snapshot "Index Now".

### 6. Entry point — `src/pages/RepositoriesPage.tsx`
Add a **"Search Files…"** item to the repo context menu, right after
"Open Snapshots" (`RepositoriesPage.tsx:829-834`), navigating to
`/snapshots/${contextMenu.repo.id}/search`. (Optional: a matching inline
search-icon ghost button in the repo row alongside the existing row actions
at `:422-490` — include for discoverability, matching SnapshotsPage's per-row
search button style.)

## Critical files
- `src-tauri/src/commands/cache.rs` — `search_repo_files` DB method (+ near `:803`)
- `src-tauri/src/commands/browse.rs` — `search_repo_files` command + `RepoFileHit` struct
- `src-tauri/src/lib.rs` — register command
- `src/lib/invoke.ts`, `src/lib/types.ts` — wrapper + type
- `src/pages/RepoSearchPage.tsx` — new page (patterned on `SearchPage.tsx`)
- `src/App.tsx` — new route
- `src/pages/RepositoriesPage.tsx` — context-menu entry point

## Verification
1. `npm run test:rust` — add a Rust unit test in `cache.rs` (alongside existing
   browse-cache tests, e.g. near `:1675`): seed two snapshots of the same repo
   with overlapping paths + a `complete` status each, assert `search_repo_files`
   returns each path once and resolves to the snapshot with the greater `time`.
   Also assert a `pending`/`in_progress` snapshot's files are excluded.
2. `npm run tauri dev`, then: right-click a repo → **Search Files…**; with a
   partially-indexed repo confirm the "N of M" banner + **Index All**; type a
   query and confirm deduped results with the correct (newest) snapshot chip;
   click a result → lands in that snapshot's BrowsePage at the right directory;
   press **← Search** → returns here with query + results intact.
3. Edge cases: repo with 0 indexed snapshots (Index All CTA), empty query
   (cleared results), no matches (empty state), LIKE metacharacters (`%`, `_`)
   treated literally.
