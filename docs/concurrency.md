# Concurrency & the Operation Event Bus

Read this before: adding/changing anything touching `RepoLocks`, the `task` event bus,
cancellable operations, or any command that shells out to restic more than once at a time.
Linked from CLAUDE.md's top-level Concurrency and Operation Event Bus summaries.

## Concurrency: Per-Repository Lock Registry

Restic distinguishes **shared** locks (most commands — `backup`, `restore`, `copy`, `mirror`'s
`copy`, `check`, `snapshots`, `stats`, `ls`) from **exclusive** locks (`forget`, `prune`, `tag` —
nothing else may touch the repo while one runs). The app had no cross-operation awareness of
this, so an exclusive op could fire mid-shared-op and fail with restic's own "repository is
already locked by PID …" — reproduced by starting a manual backup, clicking Refresh on
Repositories (`refresh_repo_stats`) while it runs, and watching the backup's post-run retention
collide with that still-in-flight `stats` call.

`RepoLocks` (`src-tauri/src/commands/repo_locks.rs`, managed state) is an in-memory
`HashMap<repo_path, {readers, exclusive}>` — keyed by **repository path** (restic's true lock
identity, so two `repo_id`s pointing at the same path correctly serialize), not `repo_id`. Two
RAII guards, both releasing on `Drop`:

- **`ReadGuard`** (`RepoLocks::read`) — shared-lock ops acquire this. **Never blocks**; just
  increments a counter and returns immediately. Readers are deliberately one-directional — they
  never wait for writers — so a slow exclusive op can't make a listing/stats call hang.
- **`WriteGuard`** (`RepoLocks::write` async / `write_blocking` sync) — exclusive-lock ops
  acquire this. Polls until the repo has zero readers and isn't already exclusive, then
  atomically claims it — **waits genuinely, no timeout or force-claim.** An earlier version
  force-claimed after 15s, which reintroduced the exact collision this registry exists to
  prevent whenever the shared op it waited on ran longer than 15s — a confirmed regression, not
  hypothetical. Restic's own lock, plus the retry below, remain the backstop for a genuine
  residual collision (e.g. an external restic/cron process `RepoLocks` can't see).

Wired into every shared-lock op (`execute_backup`, `restore_snapshot`, `restore_path`,
`copy_snapshot`/`mirror_repo` — both src **and** dest — `refresh_snapshots`,
`get_snapshot_stats`, `diff_snapshots`, `refresh_repo_stats`, `check_repo`,
`list_files`, `run_full_index` — shared by `index_snapshot`/`index_snapshots_batch` **and** the
`cache_warmer` auto-sweep) and every exclusive-lock op (`delete_snapshot`, `tag_snapshot`,
`prune_repo`/`prune_all_repos`, `apply_retention` — covering all three callers: `forget_by_plan`,
the scheduler tick, `run_schedule_now`). For a streaming op the guard is a local held across the
`spawn_blocking(...).await`, claimed for the whole child-process lifetime. `mirror_repo` is the one
exception to "acquired in the outer command body": since it queues and returns its `operationId`
immediately (see Restic Integration), its guards are acquired *inside* the detached `spawn`ed task,
right after it wins its turn on `MirrorHandle::turn` — the outer command has already returned by
the time a guard would otherwise be needed. `restic unlock` calls (cancel paths, `unlock_app`) are
**exempt** — they're the recovery mechanism and must never wait.

`prune_repo`/`prune_all_repos` re-check `PruneHandle::cancelled` right after acquiring their
write guard and before spawning the child — `write()`'s wait has no cancellation hook, so
without this a Stop click during that wait would leave an unkillable orphaned `restic prune`
running while the app reported "Cancelled". A second, narrower race exists at the moment the
child is stored: a concurrent `cancel_prune` between `spawn()` returning and the child landing in
`PruneHandle::child` would see `None` and no-op. `run_one_prune_attempt`'s polling loop makes its
own kill attempt the moment it observes `cancelled`, closing the gap regardless of which side of
the race fired (killing an already-exited child is a harmless no-op).

`RepoLocks` only coordinates this app's own operations — it can't see a different machine or tool
(restic CLI, Backrest, another Resty Desktop instance) genuinely holding the repo's real restic
lock. All four exclusive-lock commands (`delete_snapshot`, `tag_snapshot` via
`run_restic_blocking_retrying_on_lock`; `prune_repo`/`prune_all_repos` via
`run_one_prune_attempt`) retry up to twice, 2s apart, on restic's own "already locked" error
before surfacing it, matching `apply_retention`'s original retry pattern. `prune_repo`/
`prune_all_repos` capture stderr for this (previously discarded via `Stdio::null()`), so a prune
failure surfaces restic's actual error text instead of a generic "Prune failed".

Coverage is intentionally partial-safe: since writers only wait on the `readers` counter (never
the reverse), an un-instrumented reader just degrades to pre-`RepoLocks` behavior for that
pairing — it can't introduce a new failure, only leave one collision un-prevented. Don't
"complete" this by making readers wait for writers too — a slow exclusive op on a large/remote
repo would then make snapshot listings and stats hang, a worse regression than the rare
collision this registry exists to prevent.

## Operation Event Bus

`src-tauri/src/tasks.rs` defines a second, **uniform** event layer on top of the ad-hoc
per-operation events described above (`backup:progress`, `restore:progress`,
`scheduler:*`). Those events grew one at a time, so their payloads are
inconsistent — some carry no id at all, some only a display name, and roughly half the restic
operations (`copy`, `mirror`, single-repo `prune`, `forget`/retention, `check`, `diff`,
`restore_path`, `unlock`) emit nothing. The `task` event bus exists so every operation reports a
consistent, correlatable lifecycle — **in addition to**, never instead of, its existing detailed
feed — so a future background-task consumer has a uniform contract to build on instead of
retrofitting every operation at that point.

**Two layers, deliberately kept separate:**
- **`task` (this bus)** — one Tauri event, uniform envelope, every covered operation. This is the
  coordination layer a future subscriber uses.
- **Existing detailed feeds** (`backup:progress`, etc.) — unchanged, still power every shipped
  modal and the Activity panel. Rich, per-kind detail (current file names, ETA) lives here, not in
  the normalized `task` envelope.

**Envelope** (`TaskEvent`, camelCase over the wire):
`operationId` (unique per operation instance — see below), `kind` (`backup`|`restore`|
`restorePath`|`copy`|`mirror`|`prune`|`forget`|`tag`|`check`|`diff`|`index`|`unlock`|`stats`|
`testConnection`|`browse`|`init`), `phase`
(`started`|`progress`|`cancelling`|`cancelled`|`finished`|`failed`), `repoId`, `targetId`
(plan/snapshot/schedule id, when one applies), `origin` (`manual`|`scheduler`|`background`),
`progress` (normalized `percentDone`/`itemsDone`/`itemsTotal`/`bytesDone`/`bytesTotal`/`label`,
plus `secondsElapsed`/`secondsRemaining`/`currentFiles`/`repoId` — per-kind detail kept lossless
vs the legacy `backup:progress`/`restore:progress` payloads even though no consumer reads it yet
(`currentFiles`/`secondsRemaining` are backup-only, `repoId` is prune-all's per-tick repo,
distinct from the envelope's own `repoId` which is `""` for a multi-repo prune) — only on
`phase: progress`), `error` (only on `phase: failed`), `at` (unix millis).

**Why `operationId` is the core of the design, not an afterthought:** today's per-operation events
get away with carrying no id (or just a display name) only because of this app's single-in-flight
`busy` flags — one backup, one restore, one prune at a time. A future background-task system that
runs operations concurrently breaks that invariant, at which point `repoId` alone can no longer
tell two simultaneous operations apart. `operationId` (a 16-char alphanumeric id, same scheme as
`backup_history.id`) is generated once per operation and threaded through every event for its
lifetime specifically so that retrofit never has to happen.

**`OperationCtx<S: TaskSink>`** owns one operation's lifecycle: `OperationCtx::new(...)` emits
`started`; `.progress_emitter()` returns a cheap `Clone`-able `TaskProgressEmitter` for emitting
`progress` from inside a `spawn_blocking` closure (the ctx itself stays in the outer async scope
so it can read the final `Result`); exactly one of `.finished()` / `.failed(err)` / `.cancelled()`
must be called on every exit path. If none is called (an unhandled early return or panic unwind),
`Drop` emits a trailing `failed("operation dropped")` — a **backstop only**, not the intended
path; every wired call site is expected to call a terminal method explicitly (see the
`'body: { ... break 'body Err(...) }` labeled-block pattern in `prune_all_repos`/`prune_repo`,
used specifically so every one of their several early-return points still reports through
`OperationCtx` instead of falling through to the Drop backstop). `TaskSink` is a trait (implemented
for `AppHandle`) purely so `tasks.rs`'s tests can record emitted events without a real app.

Cancellable operations (backup, restore, copy, prune) carry a `current_task: TaskSlot`
(`Arc<Mutex<Option<TaskRef>>>`) on their existing handle (`BackupHandle`, `RestoreHandle`, ...) —
`OperationCtx::new` publishes its `TaskRef` (including the operation's `origin`, so
`emit_cancelling` reports the operation's real origin rather than assuming every cancel is
user-initiated) there on start and clears it on terminal; the matching `cancel_*` command calls
`emit_cancelling(&app, &handle.current_task)` right before its existing kill/stop logic, so
`cancelling` always precedes the `cancelled`/`finished` the operation itself emits once it actually
stops. Operations with no cancel path (check, diff, tag, unlock, forget, single-snapshot
`index_snapshot`) pass `None` for the slot.

The `index_snapshots_batch` ("Index All") batch is a deliberate exception to the shared-handle
`current_task` pattern above: since multiple batches (e.g. for different repos) can run
concurrently, a single `TaskSlot` on `IndexHandle` would let a second batch silently steal the
first's cancel target (a real bug this design replaced — see `IndexHandle::batches`' doc comment
in `cache.rs`). Instead, each batch creates its **own** fresh cancel flag + `TaskSlot` and
registers the pair in `IndexHandle::batches: Arc<Mutex<HashMap<operationId, BatchCancel>>>` for
its duration (deregistered on any exit via `BatchDeregisterGuard`, mirroring `ManualIndexGuard`'s
Drop pattern). `cancel_index_batch(operation_id)` looks up that specific batch's entry and calls
`emit_cancelling`/sets its cancel flag — a no-op if the batch already finished. `cancelling` only
means "no further snapshots will start" — the snapshot already in flight still finishes normally
(`finished`/`failed`), since cancellation is checked only between snapshots, never mid-`restic`.

`mirror_repo` is the second operation to move onto this same per-operation-registry pattern rather
than a shared handle slot, for the identical reason as "Index All": once queueing multiple mirrors
was added — including two into the same destination from different sources, which share a `repoId`
on the wire — a single `TaskSlot` on `MirrorHandle` could no longer identify which run a cancel or
terminal event belonged to. `MirrorHandle::mirrors: Arc<Mutex<HashMap<operationId, MirrorEntry>>>`
(`cache.rs`) mirrors `IndexHandle::batches`/`BatchCancel` field-for-field (cancel flag, `TaskSlot`,
a `cancel_notify` for a still-queued run, `started`), plus the run's own `child` handle and its
`(src_id, dest_id)` pair (for the duplicate-request guard); `MirrorHandle::turn` mirrors
`IndexHandle::batch_turn`'s FIFO lane, without a `gate` equivalent — a single `restic copy` process
has nothing to memory-bound the way concurrent `restic ls` calls did. `cancel_mirror(operation_id)`
targets one run's entry the same way `cancel_index_batch` does. Simpler than index in two ways: no
per-item loop (a mirror is one process, not N snapshots), and a queued run's cancellation is
unambiguous — a *running* mirror is killed immediately, a *queued* one never spawns at all (no
"takes effect between snapshots" caveat).

**Coverage:** every restic-shelling operation is wired, including the click-bounded metadata reads
(`refresh_repo_stats` — via the shared `fetch_and_cache_stats` helper, `not` the outer command —
`get_snapshot_stats`, `test_repo_connection`, `list_files`). Two categories are excluded, deliberately:
- **Not real restic operations at all** — `list_snapshots` (`AppDb::get_snapshots_vec`, pure cached
  DB read), `get_snapshot_index_status` (sync DB read), and `get_repo_stats` (sync, cache-only DB
  read — no restic call, ever, not even a fallback on a miss; see its doc comment in repo.rs for
  why removing that fallback was itself a fix, not just a simplification: `RepositoriesPage`
  requests this for every repo on mount, and it used to fall through to a live `restic stats` call
  on a cache miss — harmless normally, but "Clear All Cache" wipes every repo's cached stats at
  once, so the very next mount silently refreshed all of them, contradicting stats' manual-only
  design). Nothing runs that a task could represent in any of the three.
- **Continuous background work, not user-bounded** — `cache_warmer`'s `refresh_all_snapshots` tick
  (runs automatically every 60s, forever, per eligible repo, for as long as the app is open). Unlike
  every other wired operation this isn't bounded by a user action, so it was kept off the bus to
  avoid unbounded event volume over a long-running session; revisit deliberately if a future
  consumer needs ambient background activity visible too.

Any new restic-shelling command should go through `OperationCtx` unless it falls in one of those
two categories.

**Frontend scope — six stateful consumers so far (`stats`, `index`'s per-snapshot lifecycle,
`index`'s batch-level progress, the scheduler-backup `activeBackup` row, `prune`'s
`activePrune` row, and mirror's `activeMirrors`); everything else still emits into the void.**
`src/lib/types.ts`
mirrors the envelope (`TaskEvent`, `TaskKind`, `TaskPhase`,
`TaskOrigin`, `TaskProgress`) so a consumer has a ready-made contract. `ActivityProvider`
(`src/lib/activity.tsx`) subscribes to `task` filtered to `kind: "stats"` — repo stats refreshes
never had a legacy per-operation feed (the page always updated straight from the command's
promise return), so this was the first case with no existing detail feed to duplicate or
choreograph around. The subscription is deliberately **lifecycle-only, and text-free**: it tracks
`operationId → repoId` across `started`/`finished`/`failed`/`cancelled` to drive both an
in-flight indicator (`statsRefreshing` — rendered as an indeterminate `ProgressBar` row in
`ActivityPanel`, a spinning icon on `RepositoriesPage`'s own rows) and a plain boolean "last
attempt failed" marker (`statsFailed`, cleared the moment a new attempt starts or a later one
succeeds) — both surfaced in `ActivityPanel` and read directly by `RepositoriesPage`. No error *message* is ever
carried, stored, or shown; a manual refresh only needs to tell the user "that didn't work," not
restic's specific reason, so the marker is a `Set<repoId>`, never a `Map<repoId, string>`. This
is also why `fetch_and_cache_stats` (`repo.rs`) creates its `OperationCtx` **before** validating
the master key or resolving the repo, with every fallible step explicitly calling
`task_ctx.failed(e)` rather than relying on `?` — the frontend marker has no fallback to the
invoke promise's own rejection, so every failure path must reliably reach the bus (previously,
auth/repo-lookup failures emitted no task event at all, and a `parse_stats_json` failure fell
through to `OperationCtx`'s `Drop` backstop instead of an explicit call).

`index` is the second consumer, and the first case of a **legacy event fully retired** rather than
added alongside — the old `index:done` event (emitted by `index_snapshot`, `index_snapshots_batch`,
and `cache_warmer`'s auto-indexer) was removed outright once its four listeners
(`activity.tsx`, `SnapshotsPage`, `SearchPage`, `RepoSearchPage`) were ported to `task`, since the
envelope already carried a strict superset of its payload (`snapshotId`→`targetId`, `repoId`,
`success`→`phase`). Each listener filters to `kind === "index"` and a terminal phase
(`"finished"`/`"failed"`); `activity.tsx` uses it as a pure lifecycle trigger for `refreshIndexing`
(same as its unmigrated `snapshots:refreshed` listener), while the three page-level listeners read
`targetId`/`phase` directly to drive per-row index-status maps and the "Index All" batch progress
UI — a case where, unlike `stats`, the event payload itself (not just its lifecycle) is consumed.
`snapshots:refreshed` remains on the legacy path deliberately: it's `cache_warmer`'s
`refresh_all_snapshots` tick, which is excluded from the `task` bus entirely (see the coverage
exclusions above).

`index_snapshots_batch` ("Index All") additionally emits a **batch-level** `task` op alongside its
per-snapshot ones — `kind: "index"`, `origin: "manual"`, but with **no `targetId`**, which is the
only thing that distinguishes it from the per-snapshot events on the wire (see `browse.rs`'s
`index_snapshots_batch` doc comment). It reports `phase: "progress"` with `itemsDone`/`itemsTotal`
as the batch advances. Each batch owns its own cancel flag + task slot rather than sharing one
across every batch (see `IndexHandle::batches`, `cache.rs`), so `cancel_index_batch(operation_id)`
targets exactly one running batch. `ActivityProvider` (`activity.tsx`) tracks these ops via
`reduceIndexBatches` — its first case of reading `progress` off the bus rather than treating it
purely as a lifecycle signal — as a `Map<operationId, ActiveIndexBatch>` (the same shape
`StatsOpsState` already uses for concurrent stats refreshes), exposed as `activeIndexBatches: []`;
`ActivityPanel` renders **one row per active batch**, each a determinate "X / N snapshots" bar with
its own Stop button in Active Tasks. Each event only carries `repoId`, so `ActivityProvider`
separately resolves display names via a single `listRepos()` call covering the whole set of
currently-active batches' repoIds (`indexBatchRepoNames`, re-fetched whenever that set changes) —
the same by-id lookup `loadUpcoming` does for plan names, just async since a batch can start at any
time; falls back to a repo-less label per batch if the lookup fails or that repo was deleted
mid-batch. This was deliberately the *first* exception to "restore/copy/manual backup already have
their own progress modals and are intentionally excluded" (see `ActivityPanel.tsx`'s header
comment) — prune and mirror have since followed the same pattern, see their own paragraphs below.
"Index All"'s modal — `RepoSearchPage`, and independently `RepositoriesPage`'s
context-menu equivalent — is explicitly dismissible while its batch keeps running, so unlike those
other modals it needs a way to stay visible and cancellable after the modal closes — each page's own
Stop button captures its batch's `operationId` from the same `started` task event (see the page's
`task` listener) so it targets only its own batch, independent of any other batch running elsewhere
(including one started from the *other* page — each page's `getActiveIndexBatch` call adopts an
already-running batch for the same repo instead of rejecting/duplicating). The existing per-snapshot
listeners (`SnapshotsPage`/`SearchPage`/`RepoSearchPage`) already guard on `targetId` being set, so they
transparently ignore every batch-level op with no changes required.

`activeBackup` (the scheduler-triggered backup row in Active Tasks) is the fourth consumer, and the
first case of a **legacy event family fully retired** for a multi-op lifecycle rather than a
single event (`index:done` was one event; this replaced four: `scheduler:backup-started`,
the guarded `backup:progress`, `scheduler:retention-started`, `scheduler:backup-finished`).
`reduceSchedulerBackup` (`activity.tsx`) filters `task` to `origin: "scheduler"` and stitches two
separate ops into one continuous row across a plan's full run: `kind: "backup"` (started/progress/
finished, `targetId` = plan id) for the transfer phase, then `kind: "forget"` (started/finished,
same `targetId`, matched only once the backup op has reached `finished`) for retention — the same
two ops `execute_backup`/`apply_retention` already emit via their own `OperationCtx`s (see
scheduler.rs), just filtered and correlated on the frontend rather than the backend re-emitting a
combined signal. A plan with no retention configured never gets a `forget` op (`scheduler.rs` only
calls `apply_retention` when the plan has ≥1 `keep_*` flag set), so the reducer alone can't decide
when to dismiss that case — it leaves the row sitting with a `backupFinished: true` marker, and a
separate provider effect (keyed on the run and re-checked when `backupFinished` flips) resolves the
plan via `listBackupPlans()` and clears the row once it confirms no retention is coming. That same
lookup also resolves the row's display name — the bus carries only the plan id, not a display name,
same as `indexBatchRepoNames` above — so the row shows plan name only (the legacy events additionally
carried a schedule name that the bus has no equivalent for). This same fix-the-early-return-gap
pattern used by `fetch_and_cache_stats` above was needed in `apply_retention` too, for the identical
reason: it originally did `master_key.get()?`/`db.get_full_repo(...)?` before creating its
`OperationCtx`, so a failure there (e.g. the plan's repo deleted in the narrow window between backup
and retention) skipped emitting any `forget` event at all — with the legacy `scheduler:backup-finished`
retired, nothing else would have cleared the row, leaving it stuck until some unrelated later
scheduled backup happened to displace it. `apply_retention` now creates its `OperationCtx` first and
calls `task_ctx.failed(e)` explicitly on both lookups, closing that gap the same way `repo.rs` already
had for stats.

`activePrune` (the "Pruning repositories" row in Active Tasks) is the fifth consumer, and —
like `index`'s batch-level progress — reads real `progress` off the bus rather than treating it
purely as a lifecycle signal. `reducePrune` (`activity.tsx`) is a single nullable slot, not a
`Map`, since prune is single-in-flight app-wide (`PruneHandle`'s `busy` guard) unlike concurrent
index batches. It covers both `prune_all_repos` (progress-bearing: `itemsDone`/`itemsTotal`/
`label` per repo — emitted twice per repo, once before that repo's prune starts and once after it
finishes, mirroring `index_snapshots_batch`'s already-correct `i + 1` pattern; the pre-work emit
alone would report `N - 1 of N` on the batch's final repo, since it only ever names the repo
currently *starting*, never the one that just finished) and single-repo `prune_repo`
(lifecycle-only — `itemsTotal` stays `0`, so the
row renders the shared `ProgressBar` component's `indeterminate` (constantly-sliding) mode
rather than a determinate bar stuck at 0%). This is also the first case of a legacy event retired **after** its
one remaining consumer was ported on its own, unprompted by a wider rewrite: the legacy
`prune:progress` event existed solely to feed `SettingsPage`'s "Prune All Repositories" modal
(its progress bar, "Pruning `<repo>` (n of N)…" text, and repo count), which now mirrors the
same `activePrune` state the Activity panel already reads, gated on that modal's own local
`pruning` flag so a concurrent single-repo prune sharing the slot can't overwrite its numbers.
Once ported, the `app.emit("prune:progress", ...)` calls and their `PruneProgress` struct
(`repo.rs`) were deleted outright — the same "retire once ported" treatment as `index:done` and
the `scheduler:*` events, just triggered by a single-consumer migration rather than a four- or
five-listener one.

`activeMirrors` is the sixth consumer, and — unlike every consumer above it — required no
"port a legacy event" or "read `progress` for the first time" story: `mirror_repo` already emitted
`started`/`finished`/`cancelled`/`failed` via `OperationCtx` before it gained a modal-visible
Activity-panel row, since it emits no `progress` at all (restic `copy` streams nothing incremental,
so a running row is always an indeterminate `ProgressBar`, never `X`-of-`N`). What's new is the
*queueing*: `reduceMirror` (`activity.tsx`) is a `Map<operationId, ActiveMirror>` — the same shape
`reduceIndexBatches` uses, and for the identical reason — since `mirror_repo` allows multiple runs
to be queued at once, including two into the same destination from different sources, which share
a `repoId` on the wire; a single nullable slot (like `activePrune`, correct only because prune is
single-in-flight app-wide) would conflate them. `reduceMirror` needs no `origin`/`targetId` guard
the way `reduceIndexBatches` does to isolate batch-level ops from per-snapshot ones — mirror always
emits `origin: "manual"` and never sets `targetId` (it copies every snapshot, not one), so
`kind === "mirror"` alone is enough, the same single-guard shape `reducePrune` uses. `ActivityPanel`
renders one row per running mirror in Active Tasks and one per queued mirror in "Up Next" — the
same running/queued split `activeIndexBatches` already renders — each independently stoppable via
`cancelMirror(operationId)`. Enabling this consumer *did* change a backend signature, unlike every
consumer above it: `mirror_repo` now returns its `operationId` immediately (queued, fire-and-forget)
rather than blocking until the copy finishes, and `cancel_mirror` now takes that `operationId`
rather than targeting a single shared handle — see the mirror paragraph earlier in this section and
Restic Integration's `mirror_repo` bullet for the full backend design. `RepositoriesPage`'s own
mirror modal reads the same `activeMirrors` state directly (matched by `operationId`) to render its
queued/running phase live, rather than duplicating that state locally — see its doc comment in the
Project Structure tree above for the full modal state machine, including the backstop that infers a
terminal outcome from `activeMirrors` dropping the id, for the narrow case where the modal's own
dedicated listener loses a race with an unusually fast mirror.

The *data* (the actual `ResticStats` numbers) never rides the event either — a consumer hears
`finished` and re-reads `get_repo_stats` (a guaranteed cache hit, since `fetch_and_cache_stats`
writes `repo_stats_cache` before it calls `task_ctx.finished()`), rather than widening the
envelope with a result payload. That ordering (cache write before `finished`) is intentional —
it makes "task says finished" provably imply "cache read will see the new value," not just
usually true.

`backup`/`forget` are now consumed too, but only partially — `reduceSchedulerBackup` filters to
`origin: "scheduler"`, so manual/"Run Now" backups and manual retention (`forget_by_plan`,
`run_schedule_now`'s retention call) still emit into the void for Activity-panel purposes; they
already have their own progress modals per the "Restore/copy/manual backup" exclusion above (`mirror`
is no longer part of that list — see its consumer paragraph above). For every other kind (`restore`,
`copy`, …) **no stateful frontend code subscribes to `task`** at all yet — that remains deliberate,
not an oversight: a live consumer wired before
there's an actual feature needing it risks the same fate as an earlier, scrapped attempt at this
pattern (over-eager re-renders, a shape that rots before it's ever exercised). `App.tsx`'s dev-only
`console.debug("[task]", ...)` effect still covers those — stateless (never calls `setState`),
gated on `import.meta.env.DEV`, safe to delete. The floor against "emitting into the void" for
the rest is `tasks.rs`'s own test suite (a recording `TaskSink` asserting lifecycle ordering and
the exact camelCase JSON shape) plus the shared TypeScript types keeping the two sides in sync.

