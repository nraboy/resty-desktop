# Optimization Pass — Safe Wins (Detailed Implementation)

## Context

The app is in solid shape with strong UX feedback. This pass makes performance, storage, and
code-quality improvements **without changing or regressing any feature**. Findings came from a
verified backend + frontend audit. Scope chosen by the user: **safe wins only** — every change
is behavior-preserving, needs **no schema migration**, and adds **no new dependency**. Build
time is explicitly acceptable to trade for a better runtime binary.

The central theme: several `async fn` Tauri commands run a blocking `restic` subprocess (or a
JSON round-trip) **inline on a tokio async-runtime worker thread**. Tauri polls async commands
on a multi-threaded runtime whose workers are shared with every other async command and with
the single `AppDb` `Mutex<Connection>`; a blocked worker starves them (the exact hazard in
CLAUDE.md "Persistence & Caching"). Streaming commands already fix this with `spawn_blocking`;
the one-shot commands don't. **Note:** *sync* `#[tauri::command]`s (e.g. `get_restic_version`,
`list_repos`) run on Tauri's own thread pool, not the async runtime, so they are **not** in
scope — only `async fn` commands that block are.

---

## Backend

### B1. Move one-shot restic subprocess calls off the async runtime (highest impact)

**B1a. Add a reusable helper** in `src-tauri/src/commands/repo.rs`, right after
`run_restic_with_path` (ends line 33):

```rust
/// One-shot restic on a blocking-pool thread so it never occupies an async-runtime
/// worker. Owns its inputs so they can cross the spawn_blocking boundary.
pub(crate) async fn run_restic_blocking(
    repo: FullRepository,
    args: Vec<String>,
    restic_path: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_restic_with_path(&repo, arg_refs, &restic_path)
    })
    .await
    .map_err(|e| e.to_string())?
}
```

**B1b. Make `FullRepository` cloneable** (`cache.rs:83`) so commands that issue two restic calls
(e.g. `tag_snapshot`) can hand an owned copy to each. Each clone still zeroizes on drop:
```rust
#[derive(Clone, ZeroizeOnDrop)]   // was: #[derive(ZeroizeOnDrop)]
pub struct FullRepository {
```

**B1c. Convert each async command below.** Pattern: keep the fast setup (`master_key.get()`,
`db.get_full_repo`, `get_restic_path`) inline, then replace the inline
`run_restic_with_path(&repo, vec![...], &restic_path)?` with
`run_restic_blocking(repo, vec![...owned Strings...], restic_path).await?`. Move `repo` in when
it isn't reused after; `repo.clone()` / `restic_path.clone()` when it is.

| Command | Location | Owned args to pass |
|---|---|---|
| `refresh_snapshots` | `snapshot.rs:54` | `vec!["snapshots".into(), "--json".into()]` (move `repo`) |
| `delete_snapshot` | `snapshot.rs:72-76` | build `vec!["forget".into(), snapshot_id.clone()]`, push `"--prune".into()` when `prune` |
| `tag_snapshot` | `snapshot.rs:99,107` | `vec!["tag".into(),"--add".into(), tag_str, snapshot_id.clone()]` and `…"--remove"…`; use `repo.clone()`/`restic_path.clone()` on each branch |
| `diff_snapshots` | `snapshot.rs:162` | `vec!["diff".into(), snapshot_a.clone(), snapshot_b.clone()]` |
| `get_snapshot_stats` | `snapshot.rs:205` | `vec!["stats".into(),"--json".into(), snapshot_id.clone()]` |
| `unlock_repo` | `snapshot.rs:708` | `vec!["unlock".into()]` |
| `list_files` | `browse.rs:82` | convert the existing `args: Vec<&str>` (built just above) to a `Vec<String>` and move it in |
| `restore_path` | `browse.rs:114` | same: build the arg list as `Vec<String>` and move it in |
| `test_repo_connection` | `repo.rs:139` | `run_restic_blocking(dummy, vec!["snapshots".into(),"--json".into()], restic_path).await.map(\|_\| ())` |

**B1d. `init_repo` (`repo.rs:112`) — replace the raw `Command::new` block** (it only needs
success/failure): `run_restic_blocking(dummy, vec!["init".into()], restic_path).await.map(|_| ())?;`
then keep the existing `encrypt` + `db.add_repo`.

**B1e. `check_repo` (`repo.rs:202-211`) — needs the full `Output`** (parses stdout+stderr and
inspects exit status), so wrap only the process call, keep parsing after the await:
```rust
let repo_c = repo.clone();
let rp = restic_path.clone();
let (output, duration_seconds) = tauri::async_runtime::spawn_blocking(move || {
    let started = std::time::Instant::now();
    let output = std::process::Command::new(&rp)
        .args(["check", "--json"])
        .env("RESTIC_REPOSITORY", &repo_c.path)
        .env("RESTIC_PASSWORD", &repo_c.password)
        .no_console().augment_path().output()
        .map_err(|e| format!("Failed to run restic: {e}"))?;
    Ok::<_, String>((output, started.elapsed().as_secs_f64()))
}).await.map_err(|e| e.to_string())??;
```
(`std::process::Output` is `Send`; the existing `stdout`/`stderr`/`errors` parsing below is
CPU-only and stays on the async thread unchanged.)

**B1f. Repo stats path — make `fetch_and_cache_stats` async** (`repo.rs:164`). It's a sync helper
called by the async `get_repo_stats`/`refresh_repo_stats`. Change signature to
`async fn fetch_and_cache_stats(...)`; replace the inline `run_restic_with_path` (`:172`) with
`run_restic_blocking(repo, vec!["stats".into(),"--json".into()], restic_path).await?`; keep the
last-line parse + `db.set_stats` after. Update the two callers to `...await` (they already `await`
nothing else, so just add `.await` on the call at `:151` and `:161`).

**B1g. `apply_retention` / `forget_by_plan` (the one item with real plumbing).**
`apply_retention` (`snapshot.rs:752`) is a **sync** `pub fn` with three callers: `forget_by_plan`
(foreground async command), and the two background tasks `scheduler.rs:93` and
`schedule.rs:117`. Scope decision:
- **Convert only `forget_by_plan`** (the foreground command). Give it `app: tauri::AppHandle`,
  clone the inputs into a `spawn_blocking` that re-fetches state and calls the existing sync
  `apply_retention` unchanged:
  ```rust
  #[tauri::command]
  pub async fn forget_by_plan(
      app: tauri::AppHandle,
      repo_id: String, tags: Vec<String>, paths: Vec<String>, retention: RetentionPolicy,
  ) -> Result<String, String> {
      tauri::async_runtime::spawn_blocking(move || {
          let db = app.state::<AppDb>();
          let mk = app.state::<MasterKey>();
          apply_retention(db.inner(), mk.inner(), &repo_id, &tags, &paths, &retention)
      }).await.map_err(|e| e.to_string())?
  }
  ```
  (`RetentionPolicy` already derives `Clone`/`Deserialize`; it is moved, not cloned. Drop the now
  unused `db`/`master_key` `State` params from the signature — the invoke wrapper takes only the
  data args, so `src/lib/invoke.ts` is unaffected.)
- **Leave `apply_retention` itself sync and leave the scheduler/schedule.rs callers as-is.** They
  run inside their own `tauri::async_runtime::spawn(async move {…})` background tasks (not
  foreground commands), immediately after `execute_backup` which already did its heavy work via
  `spawn_blocking`; blocking a background task briefly does not starve user-facing commands, and
  touching the race-sensitive scheduler for marginal benefit is not worth the risk in this pass.

**Do NOT touch** the restic calls already inside `spawn_blocking`/streaming children: the cancel
→ `unlock` calls (`snapshot.rs:394,424,580,581,682,683`, `repo.rs:394,471`), the copy/mirror/
backup/prune/restore children, and `run_full_index` (`browse.rs:282`, called via `spawn_blocking`
from the warmer). Keep the `use super::repo::run_restic_with_path;` imports — still used by those
closures.

### B2. Fold `is_fully_indexed` into the single locked scope in `AppDb::get`
`cache.rs:748-751`: `get()` calls `is_fully_indexed()` (which locks, queries, unlocks) then
re-locks at `:750` — two lock acquisitions per directory read on the browse hot path. Inline the
status query under the one `conn.lock()` obtained at `:750`:
```rust
let conn = self.conn.lock().map_err(|e| e.to_string())?;
let fully_indexed = matches!(
    conn.query_row(
        "SELECT 1 FROM browse_cache_status WHERE repo_id = ?1 AND snapshot_id = ?2 AND status = 'complete'",
        params![repo_id, snapshot_id], |_| Ok(())),
    Ok(())
);
let snap = Self::snap_id_of(&conn, snapshot_id)?;
// …rest unchanged…
```
Keep the standalone `is_fully_indexed` method (other callers may exist); just stop calling it from
`get`. (Behavior identical: a genuine DB error there previously propagated; `matches!(… , Ok(()))`
treats an error as "not fully indexed" — acceptable, but if strict parity is wanted, keep the
explicit `match` returning `Err` on `Err(e)`. **Use the explicit `match` form to preserve exact
error semantics.**)

### B3. `prepare_cached` in the bulk-insert loops
Replace per-row `tx.execute(sql, …)` / `conn.execute(sql, …)` inside loops with one
`prepare_cached` statement reused across the loop. Applies to:
- `insert_browse_files` `cache.rs:993` — `let mut stmt = tx.prepare_cached(INSERT_SQL)?;` before
  the `for entry in chunk` loop, then `stmt.execute(params![...])`.
- `set_snapshots` `cache.rs:1118` and `append_snapshots` `cache.rs:1147` — same, one
  `prepare_cached` before the row loop. (`append_snapshots` uses `conn`, not a `tx`; `prepare_cached`
  works on `Connection` too.)
- `set` `cache.rs:792` (browse-file setter) — same pattern.
Behavior-identical; removes tens of thousands of statement recompiles on large snapshot indexing.

### B4. Drop the JSON serialize→parse round-trip on the snapshot-list path
`get_snapshots` (`cache.rs:1050`) builds a JSON string that `list_snapshots` (`snapshot.rs:37`)
immediately re-parses into `Vec<Snapshot>`. `get_snapshots` is verified to have **exactly one
caller**. Add a direct method (import `use super::snapshot::Snapshot;` at top of `cache.rs`):
```rust
pub fn get_snapshots_vec(&self, repo_id: &str) -> Result<Vec<Snapshot>, String> {
    let conn = self.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(
        "SELECT snapshot_id, short_id, time, hostname, username, paths, tags
         FROM snapshots_cache WHERE repo_id = ?1 ORDER BY time ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![repo_id], |row| {
        let paths: String = row.get(5)?;
        let tags: Option<String> = row.get(6)?;
        Ok(Snapshot {
            id: row.get(0)?, short_id: row.get(1)?, time: row.get(2)?,
            hostname: row.get(3)?, username: row.get(4)?,
            paths: serde_json::from_str(&paths).unwrap_or_default(),
            tags: tags.and_then(|t| serde_json::from_str(&t).ok()),
        })
    }).map_err(|e| e.to_string())?
      .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(rows)
}
```
Then `list_snapshots` becomes `db.get_snapshots_vec(&repo_id)` (drop the `serde_json::from_str`).
Remove the now-dead `get_snapshots` (String) method. Keep this call inline (typical snapshot
counts are small; the round-trip removal is the win — no `spawn_blocking` needed here).

### B5. Index for `backup_history`; guarded trim
- **Add the index** to `init_schema` (`cache.rs`, in the `execute_batch` block near `:319`):
  `CREATE INDEX IF NOT EXISTS idx_history_started ON backup_history(started_at);` — idempotent, no
  `user_version` bump. This is the safe core; speeds up both the after-insert trim and
  `list_backup_history`'s `ORDER BY started_at DESC`.
- **Guard the trim** in `log_backup` (`cache.rs:1304`): only run the `DELETE` when over the cap,
  preserving the documented "capped at 1000" invariant:
  ```rust
  let count: i64 = conn.query_row("SELECT COUNT(*) FROM backup_history", [], |r| r.get(0))
      .map_err(|e| e.to_string())?;
  if count > BACKUP_HISTORY_LIMIT {
      conn.execute("DELETE FROM backup_history WHERE id NOT IN (
          SELECT id FROM backup_history ORDER BY started_at DESC LIMIT ?1)",
          params![BACKUP_HISTORY_LIMIT]).map_err(|e| e.to_string())?;
  }
  ```
  (`BACKUP_HISTORY_LIMIT` is `i64 = 1000`, so the comparison is type-clean.)

### B6. Wrap `remove_repo`'s cascade deletes in a transaction
`cache.rs:446-491`: six sequential `DELETE`s under one lock, no transaction — a mid-way failure
leaves partial cleanup. Change `let conn` → `let mut conn`, open `let tx = conn.transaction()?;`,
switch the six `conn.execute` calls to `tx.execute`, end with `tx.commit()?`. Matches the existing
`clean_cache`/`reset_all` pattern; behavior-identical on the success path.

### B7. Cargo release profile — optimize hard (build time acceptable)
`src-tauri/Cargo.toml` has no `[profile.release]`. Add:
```toml
[profile.release]
strip = true
lto = true
codegen-units = 1
```
All three are runtime-behavior-identical (smaller/faster binary; longer compile). Leave
`opt-level` at the release default (`3`). **Do NOT add `panic = "abort"`** — the code is written to
survive worker-thread panics (`spawn_blocking` results handled with `.unwrap_or(false)`, `AppDb`
`Mutex` poison mapped to recoverable `Err`); `abort` would turn a survivable background-thread
panic into a full-app crash. Keep unwinding.

### B8. (Minor) Fetch `restic_path` once per cache-warmer sweep
`cache_warmer.rs:66` and `:174` call `get_restic_path` (a settings lock+query) inside per-repo /
per-snapshot loops. Fetch once at the top of `refresh_all_snapshots` / `index_next` and pass the
`String` down. Small, safe. Low priority — do only if convenient.

---

## Frontend

### F1. Stop SnapshotsPage from refreshing snapshots twice on mount
`SnapshotsPage.tsx:87-90,147-167`: `remoteAutoRefresh` loads async, `load` depends on it, so
`load` runs once with stale `false` then again when it resolves — for local repos that's **two**
`refreshSnapshots` restic calls per visit.

Fix (keeps instant cache paint + a single, correct refresh):
1. Add state `const [settingsReady, setSettingsReady] = useState(false);`. In the mount effect
   (`:87-90`), set it after the setting resolves: `getRemoteAutoRefresh().then(setRemoteAutoRefresh).catch(() => {}).finally(() => setSettingsReady(true));`
2. Reduce `load` to the **cache paint only**, deps `[repoId, repo]` (drop `remoteAutoRefresh`,
   `willRefresh`, `setRefreshing`):
   ```tsx
   const load = useCallback(async () => {
     if (!repoId || !repo) return;
     setLoading(true);
     try { const cached = await listSnapshots(repoId); setSnapshots(cached.reverse()); }
     finally { setLoading(false); }
   }, [repoId, repo]);
   useEffect(() => { load(); }, [load]);
   ```
3. Add a **separate refresh effect** gated on `settingsReady` so it fires once with the final
   setting value:
   ```tsx
   useEffect(() => {
     if (!repoId || !repo || !settingsReady) return;
     if (isRemoteRepo(repo.path) && !remoteAutoRefresh) return;
     setRefreshing(true);
     refreshSnapshots(repoId)
       .then((data) => setSnapshots(data.reverse()))
       .catch(() => {})
       .finally(() => setRefreshing(false));
   }, [repoId, repo, settingsReady, remoteAutoRefresh]);
   ```
   `settingsReady` flips false→true exactly once (with `remoteAutoRefresh` already final), so each
   repo visit/switch triggers exactly one refresh; the manual `refresh` button (`:133`) is
   untouched; remote auto-refresh still honored.

### F2. Dependency arrays on the indeterminate-checkbox effects
Both set `ref.indeterminate` in a `useEffect` with **no** dep array (runs every render):
- `SnapshotsPage.tsx:376-380` → add deps `[somePageSelected, allPageSelected]`.
- `BackupPlansPage.tsx:109-113` → add deps `[someSelected, allSelected]`.

### F3. (Minor) Memoize `otherRepos`; hoist `filter.toLowerCase()`
- `SnapshotsPage.tsx:421` `const otherRepos = allRepos.filter((r) => r.id !== repoId);` → wrap in
  `useMemo(() => allRepos.filter((r) => r.id !== repoId), [allRepos, repoId])` (used in 6 places).
- Inside the `filtered` memo (`:349-357`), compute `const f = filter.toLowerCase()` once and reuse
  it instead of calling `filter.toLowerCase()` up to 3× per snapshot.

---

## Explicitly NOT in scope (deferred / dropped after verification)
- **FTS5/trigram search index** (needs migration) — deferred by scope.
- **Shared `useDebouncedSearch` hook, extracted `FileIcon`/`browseTarget`, repos cache/context,
  BrowsePage virtualization (new dep), `React.lazy` route splitting, `CheckResultModal`** —
  structural refactors, deferred by scope.
- **`RepoSearchPage` `index:done` listener re-subscribe fix** — dropped: it lives in a search page,
  and the scope is "leave search alone." Low-risk; revisit later.
- **Skipping remote repos in `fetchStatsForLocal`** — dropped as unsafe. `get_repo_stats`
  (`repo.rs:143`) returns cached stats when present and only shells out on a miss; the Repositories
  page displays those cached values, so skipping remotes would hide cached remote stats — a
  regression. A safe version needs a cache-only path, out of scope here.
- **Converting the scheduler/`schedule.rs` `apply_retention` callers** — background tasks, left
  sync (see B1g).
- **`panic = "abort"`** and **`opt-level` size tuning** — correctness/UX exclusions (see B7).

---

## Post-implementation: update `CLAUDE.md`

Once the changes land, update `CLAUDE.md` in two ways — (A) record the new mechanics, and
(B) add an explicit "intentional designs" section so a future audit doesn't re-flag choices we
made on purpose.

### A. New context to record
- **Stack table (line 23):** `Restic integration` — note one-shot restic calls run via
  `run_restic_blocking` (spawn_blocking); never inline on the async runtime.
- **Memory safety (line 19):** `FullRepository` now also derives `Clone` (each clone still
  zeroizes on drop) so one-shot restic calls can own their repo across the `spawn_blocking`
  boundary.
- **`repo.rs` file note (line 126):** add `run_restic_blocking` (pub(crate) async helper: runs a
  one-shot restic command on a blocking-pool thread; owns its args so it can cross `spawn_blocking`).
- **`snapshot.rs` file note (line 133-134):** `list_snapshots` now returns `Vec<Snapshot>`
  straight from `AppDb::get_snapshots_vec` (no JSON round-trip); `forget_by_plan` takes an
  `AppHandle` and runs the sync `apply_retention` via `spawn_blocking`.
- **`cache.rs` file note (line 156):** mention `get_snapshots_vec` (rows→`Vec<Snapshot>` directly);
  `remove_repo` cascade is transactional; `backup_history` has `idx_history_started` and
  `log_backup` trims only when over `BACKUP_HISTORY_LIMIT`.
- **Restic Integration section:** add a bullet — "All restic subprocess calls run off the async
  runtime: streaming commands via `spawn_blocking` with a `Child`; one-shot commands via
  `run_restic_blocking`. `async fn` commands must never call `std::process::Command` inline."
- **Persistence & Caching:** update the `backup_history` bullet (line 226) to mention
  `idx_history_started` and the count-guarded trim; update the `list_snapshots` bullet (line 220)
  to note it returns structs directly via `get_snapshots_vec`.
- **New `## Build Profile` section (near Releases):** `src-tauri/Cargo.toml` sets
  `[profile.release]` `strip`/`lto`/`codegen-units = 1` for a smaller/faster binary (longer
  compile, accepted).

### B. New `## Intentional Designs (do not "optimize" these)` section
Add a dedicated section so these are not re-raised as findings:
- **Sync `#[tauri::command]`s are intentionally not `spawn_blocking`.** Tauri runs sync commands
  (e.g. `get_restic_version`, `list_repos`) on its own thread pool, off the async runtime — only
  `async fn` commands that block need `spawn_blocking`.
- **`scheduler.rs` / `schedule.rs` call the *sync* `apply_retention` directly, on purpose.** They
  run inside their own background `spawn`ed tasks (not foreground commands) right after
  `execute_backup`; only the foreground `forget_by_plan` wraps it in `spawn_blocking`.
- **`get_repo_stats` fetches stats for *all* repos incl. remote, and RepositoriesPage requests
  them on mount, on purpose.** It returns cache immediately when present and only shells out on a
  miss; the `—` fallback is for remotes with no cache. Do **not** skip remote repos — that would
  hide cached remote stats.
- **`browse_cache_files.parent_path` duplicates a prefix of `path` on every row, on purpose** — it
  backs the `(snap, parent_path)` directory-listing index (storage-for-speed; the single largest
  cache-table cost, and that's acceptable).
- **File search uses `LIKE '%query%'` (leading wildcard → full scan), knowingly.** It's why the
  search commands are `async` + `spawn_blocking` + `searchSeqRef`-guarded. An FTS5 index is a
  deliberately-deferred future improvement, not an oversight.
- **`cached_at` columns are written but not currently read** — retained for a future staleness/TTL
  feature; staleness today is handled by explicit refresh/evict. Not dead weight to be dropped
  without that feature.
- **`panic = "abort"` is deliberately NOT set** — the code survives worker-thread panics
  (`spawn_blocking` results handled, `Mutex` poison mapped to `Err`); `abort` would turn those into
  full-app crashes.
- **Known deferred (not novel) frontend cleanups:** the duplicated search/`FileIcon`/`browseTarget`
  logic across `SearchPage`/`RepoSearchPage`/`BrowsePage`, the per-keystroke `index:done`
  re-subscribe in `RepoSearchPage`, `listRepos()` refetch per page, and BrowsePage's unbounded
  list are all known — deferred by scope, revisit deliberately, don't re-discover.

## Verification

- **Tests stay green:** `npm run test:all` (Vitest + `cargo test`). Rust tests live in
  `cache.rs`/`crypto.rs`/`snapshot.rs`/`transfer.rs`. Add: a `get_snapshots_vec` test asserting it
  returns the same snapshots the old JSON path produced (round-trip a known `set_snapshots` input),
  and a `log_backup` boundary test confirming the table still caps at `BACKUP_HISTORY_LIMIT` after
  inserting >1000 rows.
- **Release build:** `npm run tauri build` succeeds; note the binary-size reduction from B7.
- **Drive the app** (`npm run tauri dev`) — behavior must be unchanged:
  - Open a local repo → cached list paints instantly, and exactly **one** restic `snapshots`
    refresh fires (not two). Switch repos → one refresh each.
  - Open a remote repo with auto-refresh **off** → no refresh; turn it **on** → refresh fires.
  - Browse a large directory → entries render (B2 single-lock path); index a snapshot, then search
    within it and repo-wide → results unchanged (search code untouched).
  - Delete a snapshot, tag a snapshot, diff two snapshots, check a repo, apply retention
    (`forget_by_plan`), get/refresh repo stats, init + test a new repo → all behave exactly as
    before (these are the `spawn_blocking`-converted commands).
  - Run a backup → Logs page shows it and stays capped at 1000 rows.
  - Delete a repo → snapshots/browse/stats caches all cleared (transaction path).
  - **Responsiveness win:** while a slow remote `check`/`refresh`/`stats` runs, confirm other repos,
    index-status polling, and the snapshot list stay responsive — the core payoff of B1.
- **Frontend:** select-all checkbox indeterminate state still correct on Snapshots and Backup
  Plans pages; no new console warnings.
