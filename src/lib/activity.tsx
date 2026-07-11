// Powers the Activity panel (src/components/ActivityPanel.tsx). Hoisted to a top-level
// provider (mounted once in App.tsx) rather than living in a page, because — unlike every
// other progress listener in this app (BackupPlansPage, SnapshotsPage, SettingsPage) — it
// must keep updating no matter which route is currently mounted. Tauri events broadcast, so
// these listeners coexist peacefully with the page-local ones that already exist.
//
// Scope is deliberately narrow: only activity the user has no other visibility into —
// background auto-indexing, scheduler-triggered backups, and (as of the `statsRefreshing`
// field below) in-flight repo stats refreshes. Restore/copy/mirror/manual backup/prune already
// have their own progress modals and are intentionally excluded here.
//
// `statsRefreshing`/`statsFailed` are this app's first stateful consumer of the unified `task`
// event bus (see CLAUDE.md's Operation Event Bus section) rather than a per-operation legacy
// feed — stats never had one (it always updated purely from the command's promise return), so
// there's no legacy event to keep alongside. The bus is used here purely as a *lifecycle*
// signal (an operationId started/finished/failed) — no error text is carried or stored;
// `statsFailed` is a plain boolean-per-repo marker, not a message, since the intended reader is
// "did my click work," not "why not" (deliberately simpler than restic's actual error text —
// see repo.rs's fetch_and_cache_stats, where every failure path explicitly reports through
// `task_ctx.failed(...)` specifically so this marker can rely on the bus alone). The actual
// stats numbers are likewise never carried on the event; RepositoriesPage re-reads them from
// the DB cache via getRepoStats, which the backend command already writes to before it emits
// `finished`.
//
// The same `task` listener also drives `refreshIndexing` on a terminal `kind: "index"` event —
// the second stateful consumer of the bus, replacing the legacy `index:done` event (removed).
// Index terminal events carry no data this effect needs (indexing progress is re-derived via
// `loadIndexing`/`getIndexProgress`, not from the event payload), so, like stats, this is a pure
// lifecycle trigger.
//
// `activeIndexBatches` is a third consumer, and the first to read the event's `progress` payload
// rather than treat the bus purely as a lifecycle signal. The manual "Index All" batch
// (index_snapshots_batch, browse.rs) emits one `task` op per snapshot (kind "index", targetId =
// snapshot id — same shape refreshIndexing reacts to above) *plus* a single batch-level op with
// no targetId, carrying `progress.itemsDone`/`itemsTotal`. The absence of `targetId` is what
// tells the two apart on the wire — see reduceIndexBatches. This is what lets a batch stay
// visible (with a Stop affordance) after its owning modal (RepoSearchPage) is dismissed or the
// user navigates away, matching activeBackup's scheduler-visibility model above.
//
// Tracked as a `Map<operationId, ActiveIndexBatch>` (mirrored into `activeIndexBatches: []` for
// consumers), the same shape `StatsOpsState` already uses for concurrent stats refreshes, rather
// than a single `ActiveIndexBatch | null` — multiple "Index All" batches (e.g. for different
// repos) can genuinely run at once; the backend gives each its own cancel flag and task slot
// (see IndexHandle::batches, cache.rs) specifically so they don't clobber each other, so the
// frontend needs to track them independently too rather than collapsing to one slot. Each
// batch's `task` events only carry `repoId`, not a display name, so `indexBatchRepoNames`
// resolves the *set* of repoIds among active batches to names via listRepos() — the same by-id
// lookup loadUpcoming already does for plan names, just async instead of inline since a batch
// can start at any time rather than only on mount/refetch.
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getAutoIndexing,
  getIndexProgress,
  listBackupHistory,
  listBackupPlans,
  listRepos,
  listSchedules,
} from "./invoke";
import type { BackupHistoryEntry, BackupProgress, Schedule, TaskEvent } from "./types";

export interface UpcomingBackup {
  scheduleId: string;
  scheduleName: string;
  planNames: string[];
  nextRunAt: number;
}

export interface ActiveScheduledBackup {
  scheduleName: string;
  planName: string;
  progress: BackupProgress | null;
  /** "backup" = bytes transferring; "retention" = the post-backup forget
   *  (apply_retention) is running. Drives the panel subtitle so the finalize
   *  step isn't mistaken for a frozen bar. See scheduler.rs's
   *  scheduler:retention-started. */
  phase: "backup" | "retention";
}

interface ActivityState {
  /** Background auto-indexing progress; null when auto-indexing is off or fully caught up. */
  indexing: { cached: number; total: number } | null;
  /** The scheduler-triggered backup currently running, if any. Manual/"Run Now" backups
   *  never populate this — see scheduler.rs's `scheduler:backup-started`. */
  activeBackup: ActiveScheduledBackup | null;
  /** Next three enabled, due schedules, soonest first. */
  upcoming: UpcomingBackup[];
  /** Last three backup history entries, newest first. */
  recentLogs: BackupHistoryEntry[];
  /** repoIds with an in-flight stats refresh (task bus kind "stats"), deduped. Populated from
   *  `task` events, not a legacy feed — see the module doc comment above. */
  statsRefreshing: string[];
  /** repoIds whose most recent stats refresh attempt ended in "failed" — cleared the moment a
   *  new attempt starts or a later attempt succeeds. No error text; see the module doc comment
   *  above for why a plain marker is the deliberate choice here. */
  statsFailed: string[];
  /** Every in-progress "Index All" batch (RepoSearchPage), across all repos — empty when none
   *  are running. Populated from each batch-level `task` op (kind "index", no targetId); see the
   *  module doc comment above and reduceIndexBatches. */
  activeIndexBatches: ActiveIndexBatch[];
  /** Display names for the repoIds among `activeIndexBatches`, resolved via listRepos() (the
   *  event only carries the id — see the module doc comment above), keyed by repoId. A missing
   *  or null entry means still resolving or the repo can no longer be found (e.g. deleted
   *  mid-batch); the panel falls back to a generic label in that case rather than a raw id. */
  indexBatchRepoNames: Record<string, string | null>;
  /** Bumped every 60s so relative-time labels ("in 3 hours") stay fresh without a refetch. */
  clockTick: number;
  /** Progress of RepositoriesPage's "Refresh Stats" (all-repos) button, or null when it isn't
   *  running. `current` is 0-indexed — the completed-so-far count, set to the loop index
   *  *before* that repo's call starts — matching SnapshotsPage's `multiDeleteProgress`/
   *  `multiCopyProgress` convention exactly rather than inventing a third one. Renderers add
   *  `+ 1` for an in-progress "working on repo N of total" label (SnapshotsPage does the same);
   *  the raw value is what a progress bar's percent should use (0% at start, 100% only once
   *  every repo has actually finished). Unlike `statsRefreshing` (derived from the `task` bus —
   *  always length 1 during this operation, since it refreshes repos one at a time, not in
   *  parallel; see RepositoriesPage's `handleRefreshAll` doc comment), this is a plain counter
   *  the page pushes here directly — there's no backend batch command to emit `task` events for
   *  the whole run the way `index_snapshots_batch` does for "Index All", since this operation is
   *  just a JS loop calling the single-repo `refresh_repo_stats` command repeatedly. Hoisted to
   *  the provider (rather than page-local state) so the panel reflects it even if the user
   *  navigates away from RepositoriesPage mid-refresh, matching `activeIndexBatches`'
   *  survive-navigation behavior. */
  statsRefreshAllProgress: { current: number; total: number } | null;
  setStatsRefreshAllProgress: (
    progress: { current: number; total: number } | null | ((prev: { current: number; total: number } | null) => { current: number; total: number } | null)
  ) => void;
}

/** A manual "Index All" batch — either actively indexing or still queued waiting its turn on
 *  the backend's batch_turn mutex — derived from its batch-level `task` op. `status` starts
 *  "queued" on a "pending" event and flips to "running" once "started" arrives (see
 *  reduceIndexBatches). */
export interface ActiveIndexBatch {
  operationId: string;
  repoId: string;
  itemsDone: number;
  itemsTotal: number;
  status: "queued" | "running";
}

/** In-flight `stats` task operations (operationId -> repoId, so concurrent refreshes — e.g.
 *  "Refresh All" — don't clobber each other) plus the set of repoIds whose latest attempt
 *  failed. Both are derived purely from the `task` bus; see the module doc comment above. */
export interface StatsOpsState {
  inFlight: Map<string, string>;
  failed: Set<string>;
}

export const initialStatsOpsState: StatsOpsState = { inFlight: new Map(), failed: new Set() };

/** Pure reducer over `stats`-kind task events. Exported for a unit test (see
 *  activity.test.ts) rather than only exercised through the provider's effect. */
export function reduceStatsOps(state: StatsOpsState, event: TaskEvent): StatsOpsState {
  if (event.kind !== "stats") return state;
  const inFlight = new Map(state.inFlight);
  const failed = new Set(state.failed);
  switch (event.phase) {
    case "started":
      inFlight.set(event.operationId, event.repoId);
      failed.delete(event.repoId); // a fresh attempt supersedes any prior failure marker
      break;
    case "finished":
      inFlight.delete(event.operationId);
      failed.delete(event.repoId);
      break;
    case "failed":
      inFlight.delete(event.operationId);
      failed.add(event.repoId);
      break;
    case "cancelled":
      inFlight.delete(event.operationId);
      break;
    default:
      break; // "progress" — stats is a single restic call, never emits this phase
  }
  return { inFlight, failed };
}

/** Pure reducer over `index`-kind task events, isolating batch-level "Index All" ops (one entry
 *  per concurrently-running batch, keyed by operationId — see the module doc comment above) from
 *  the per-snapshot ones that share the same kind. A per-snapshot op (targetId set) is ignored.
 *  Returns the same `state` reference whenever nothing changes so the caller can skip a
 *  re-render. Exported for a unit test (see activity.test.ts). */
export function reduceIndexBatches(
  state: Map<string, ActiveIndexBatch>,
  event: TaskEvent
): Map<string, ActiveIndexBatch> {
  if (event.kind !== "index" || event.origin !== "manual" || event.targetId) return state;
  switch (event.phase) {
    case "pending": {
      const next = new Map(state);
      next.set(event.operationId, {
        operationId: event.operationId,
        repoId: event.repoId,
        itemsDone: 0,
        itemsTotal: 0,
        status: "queued",
      });
      return next;
    }
    case "started": {
      const next = new Map(state);
      const existing = state.get(event.operationId);
      next.set(event.operationId, {
        operationId: event.operationId,
        repoId: event.repoId,
        itemsDone: existing?.itemsDone ?? 0,
        itemsTotal: existing?.itemsTotal ?? 0,
        status: "running",
      });
      return next;
    }
    case "progress": {
      const existing = state.get(event.operationId);
      if (!existing) return state;
      const next = new Map(state);
      next.set(event.operationId, {
        ...existing,
        itemsDone: event.progress?.itemsDone ?? existing.itemsDone,
        itemsTotal: event.progress?.itemsTotal ?? existing.itemsTotal,
      });
      return next;
    }
    case "finished":
    case "failed":
    case "cancelled": {
      if (!state.has(event.operationId)) return state;
      const next = new Map(state);
      next.delete(event.operationId);
      return next;
    }
    default:
      return state; // "cancelling" — no state change, Stop's disabled/"Stopping…" is local UI state
  }
}

const ActivityContext = createContext<ActivityState | null>(null);

export function useActivity(): ActivityState {
  const ctx = useContext(ActivityContext);
  if (!ctx) throw new Error("useActivity must be used within an ActivityProvider");
  return ctx;
}

async function loadUpcoming(): Promise<UpcomingBackup[]> {
  const [schedules, plans] = await Promise.all([listSchedules(), listBackupPlans()]);
  const planNameOf = (id: string) => plans.find((p) => p.id === id)?.name ?? id;

  return schedules
    .filter((s): s is Schedule & { nextRunAt: number } => s.enabled && s.nextRunAt != null)
    .sort((a, b) => a.nextRunAt - b.nextRunAt)
    .slice(0, 3)
    .map((s) => ({
      scheduleId: s.id,
      scheduleName: s.name,
      planNames: s.planIds.map(planNameOf),
      nextRunAt: s.nextRunAt,
    }));
}

async function loadIndexing(): Promise<{ cached: number; total: number } | null> {
  const enabled = await getAutoIndexing();
  if (!enabled) return null;
  const progress = await getIndexProgress();
  return progress.total > progress.cached ? progress : null;
}

export function ActivityProvider({ children }: { children: ReactNode }) {
  const [indexing, setIndexing] = useState<{ cached: number; total: number } | null>(null);
  const [activeBackup, setActiveBackup] = useState<ActiveScheduledBackup | null>(null);
  const [upcoming, setUpcoming] = useState<UpcomingBackup[]>([]);
  const [recentLogs, setRecentLogs] = useState<BackupHistoryEntry[]>([]);
  const [statsRefreshing, setStatsRefreshing] = useState<string[]>([]);
  const [statsFailed, setStatsFailed] = useState<string[]>([]);
  const [activeIndexBatches, setActiveIndexBatches] = useState<ActiveIndexBatch[]>([]);
  const [indexBatchRepoNames, setIndexBatchRepoNames] = useState<Record<string, string | null>>({});
  const [clockTick, setClockTick] = useState(0);
  const [statsRefreshAllProgress, setStatsRefreshAllProgress] = useState<{ current: number; total: number } | null>(null);
  // Holds name/plan between "started" and the first "backup:progress" payload.
  const pendingBackupRef = useRef<{ scheduleName: string; planName: string } | null>(null);
  // statsRefreshing/statsFailed (both deduped repoId arrays) are derived from this on every
  // "task" event via reduceStatsOps.
  const statsOpsRef = useRef<StatsOpsState>(initialStatsOpsState);
  // activeIndexBatches is derived from this on every "task" event via reduceIndexBatches;
  // mirrored into a ref (like statsOpsRef) so the listener can compare by reference and only
  // call setActiveIndexBatches when the set of batches actually changed.
  const indexBatchesRef = useRef<Map<string, ActiveIndexBatch>>(new Map());

  const refreshIndexing = () => { loadIndexing().then(setIndexing).catch(() => {}); };
  const refreshUpcoming = () => { loadUpcoming().then(setUpcoming).catch(() => {}); };
  const refreshLogs = () => { listBackupHistory().then((h) => setRecentLogs(h.slice(0, 3))).catch(() => {}); };

  useEffect(() => {
    refreshIndexing();
    refreshUpcoming();
    refreshLogs();

    const tickTimer = setInterval(() => setClockTick((t) => t + 1), 60_000);

    const unlistenSnapshotsRefreshed = listen("snapshots:refreshed", refreshIndexing);
    const unlistenSchedulesChanged = listen("schedules:changed", refreshUpcoming);

    const unlistenBackupStarted = listen<{ scheduleName: string; planName: string }>(
      "scheduler:backup-started",
      (e) => {
        pendingBackupRef.current = e.payload;
        setActiveBackup({ ...e.payload, progress: null, phase: "backup" });
      }
    );
    const unlistenBackupProgress = listen<BackupProgress>("backup:progress", (e) => {
      if (!pendingBackupRef.current) return; // not a scheduler-triggered backup — ignore
      // Functional update preserves `phase` (a plain spread of pendingBackupRef would
      // drop it). No backup:progress events arrive during retention anyway, so progress
      // stays frozen at its last value once retention-started flips the phase.
      setActiveBackup((prev) => (prev ? { ...prev, progress: e.payload } : prev));
    });
    const unlistenRetentionStarted = listen("scheduler:retention-started", () => {
      // Flip phase on the currently-active (just-backed-up) plan. Guarded so a stray
      // event with no active task is a no-op rather than fabricating one.
      setActiveBackup((prev) => (prev ? { ...prev, phase: "retention" } : prev));
    });
    const unlistenBackupFinished = listen("scheduler:backup-finished", () => {
        pendingBackupRef.current = null;
        setActiveBackup(null);
        // upcoming is refreshed via the schedules:changed event the scheduler emits
        // after record_schedule_run advances next_run_at — that fires after all plans
        // + retention complete, which is when the next fire time is actually known.
        // Refreshing here would read the stale (past) next_run_at.
      });
    // Fires for every backup, manual or scheduled (snapshot.rs's execute_backup) — unlike
    // scheduler:backup-finished above, which only covers scheduler-triggered runs and would
    // otherwise leave a manually-run backup missing from Recent Logs until the next
    // scheduled run happened to refresh it.
    const unlistenHistoryUpdated = listen("backup:history-updated", refreshLogs);

    // Subscriber to the unified `task` bus — see module doc comment above. "stats" kind
    // events drive statsRefreshing/statsFailed; "index" kind events (replacing the legacy
    // index:done) trigger a plain refetch, and each batch-level "index" op (no targetId)
    // additionally drives activeIndexBatches. Every other kind is ignored here (it's still
    // observed by App.tsx's dev-only console.debug effect).
    const unlistenTask = listen<TaskEvent>("task", (e) => {
      statsOpsRef.current = reduceStatsOps(statsOpsRef.current, e.payload);
      setStatsRefreshing([...new Set(statsOpsRef.current.inFlight.values())]);
      setStatsFailed([...statsOpsRef.current.failed]);
      if (
        e.payload.kind === "index" &&
        (e.payload.phase === "finished" || e.payload.phase === "failed")
      ) {
        refreshIndexing();
      }
      const nextBatches = reduceIndexBatches(indexBatchesRef.current, e.payload);
      if (nextBatches !== indexBatchesRef.current) {
        indexBatchesRef.current = nextBatches;
        setActiveIndexBatches([...nextBatches.values()]);
      }
    });

    return () => {
      clearInterval(tickTimer);
      unlistenSnapshotsRefreshed.then((fn) => fn());
      unlistenSchedulesChanged.then((fn) => fn());
      unlistenBackupStarted.then((fn) => fn());
      unlistenBackupProgress.then((fn) => fn());
      unlistenRetentionStarted.then((fn) => fn());
      unlistenBackupFinished.then((fn) => fn());
      unlistenHistoryUpdated.then((fn) => fn());
      unlistenTask.then((fn) => fn());
    };
  }, []);

  // The task event only carries repoId, not a display name — resolve the whole set of
  // currently-active batches' repoIds in one listRepos() call (same by-id lookup loadUpcoming
  // does for plan names above) whenever that set changes. Joined into a stable string key so the
  // effect only re-runs when the actual set of repos changes, not on every progress tick. Falls
  // back to null per id (panel shows a generic label) if a repo can't be found or the lookup
  // fails, including the rare case of a repo being deleted mid-batch.
  const batchRepoIdsKey = [...new Set(activeIndexBatches.map((b) => b.repoId))].sort().join(",");
  useEffect(() => {
    const repoIds = batchRepoIdsKey ? batchRepoIdsKey.split(",") : [];
    if (repoIds.length === 0) { setIndexBatchRepoNames({}); return; }
    let cancelled = false;
    listRepos()
      .then((repos) => {
        if (cancelled) return;
        const names: Record<string, string | null> = {};
        for (const id of repoIds) names[id] = repos.find((r) => r.id === id)?.name ?? null;
        setIndexBatchRepoNames(names);
      })
      .catch(() => {
        if (!cancelled) setIndexBatchRepoNames(Object.fromEntries(repoIds.map((id) => [id, null])));
      });
    return () => { cancelled = true; };
  }, [batchRepoIdsKey]);

  return (
    <ActivityContext.Provider
      value={{
        indexing, activeBackup, upcoming, recentLogs, statsRefreshing, statsFailed,
        activeIndexBatches, indexBatchRepoNames, clockTick,
        statsRefreshAllProgress, setStatsRefreshAllProgress,
      }}
    >
      {children}
    </ActivityContext.Provider>
  );
}
