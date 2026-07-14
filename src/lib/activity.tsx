// Powers the Activity panel (src/components/ActivityPanel.tsx). Hoisted to a top-level
// provider (mounted once in App.tsx) rather than living in a page, because — unlike every
// other progress listener in this app (BackupPlansPage, SnapshotsPage, SettingsPage) — it
// must keep updating no matter which route is currently mounted. Tauri events broadcast, so
// these listeners coexist peacefully with the page-local ones that already exist.
//
// Scope is deliberately narrow: only activity the user has no other visibility into —
// background auto-indexing, scheduler-triggered backups, and (as of the `statsRefreshing`
// field below) in-flight repo stats refreshes. Restore/copy/manual backup still have their own
// blocking progress modals and are intentionally excluded here. Prune (`activePrune` below) was
// the first deliberate exception to that exclusion, after "Index All": its Settings modal is
// dismissible, so the operation needs to stay visible/cancellable in the panel independent of
// whether that modal is open — see reducePrune's doc comment. `activeMirrors` (below) is the
// next: unlike prune (single-in-flight app-wide) or index (one batch per repo), mirror allows
// multiple runs to be queued at once — see reduceMirror's doc comment.
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
//
// `activeBackup` (the scheduler-triggered backup row) is the last "Active Tasks" consumer
// migrated onto `task`, retiring the legacy `scheduler:backup-started`/`backup:progress`
// (guarded)/`scheduler:retention-started`/`scheduler:backup-finished` events outright — the same
// full-retirement treatment `index:done` got, not an add-alongside. The bus already carries the
// whole lifecycle via `origin: "scheduler"`: `kind:"backup"` (started/progress/finished, targetId
// = plan_id) for the transfer phase, then `kind:"forget"` (started/finished, same targetId) for
// retention — see `reduceSchedulerBackup`. A plan with no retention configured never gets a
// `forget` op (scheduler.rs only emits one when the plan has ≥1 keep_* flag set), so that case is
// dismissed by the plan-lookup effect below rather than an event, since only that lookup knows
// whether retention is actually configured for the plan. The row shows the plan name only (the
// bus has no schedule name) — resolved the same lazy, cached-DB-read way `indexBatchRepoNames`
// resolves repo names.
//
// `activeMirrors` is a further consumer, tracked as a `Map<operationId, ActiveMirror>` — the
// same shape `activeIndexBatches` uses, and for the same reason: `mirror_repo` (snapshot.rs)
// deliberately allows multiple mirrors to be queued at once (even two into the same
// destination from different sources), each with its own cancel flag/task slot
// (MirrorHandle::mirrors), so a single nullable slot (like `activePrune`, which is
// single-in-flight app-wide via PruneHandle's busy flag) would lose all but one concurrent run.
// Mirror has no `progress` phase (restic `copy` streams no `--json` progress the way backup
// does), so a running row always renders as an indeterminate bar rather than X-of-N — see
// `reduceMirror`. Dest repo names are resolved by folding `activeMirrors`' repoIds into the same
// `batchRepoIdsKey` effect `indexBatchRepoNames` already uses, rather than a second resolver.
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
import type { BackupHistoryEntry, Schedule, TaskEvent, TaskProgress } from "./types";

export interface UpcomingBackup {
  scheduleId: string;
  scheduleName: string;
  planNames: string[];
  nextRunAt: number;
}

export interface ActiveScheduledBackup {
  /** Stable for the whole scheduled run (backup + retention) — set once from the backup op's
   *  operationId and never reassigned, even once `currentOperationId` adopts the forget op's id
   *  on the retention transition. Used to key the plan-lookup effect below so it only re-runs
   *  when a genuinely new run starts, not on every phase change. */
  runId: string;
  /** The operationId of whichever `task` op is currently driving this row — the backup op while
   *  `phase === "backup"`, the forget op once `phase === "retention"`. Used by the reducer to
   *  match incoming events to this row. */
  currentOperationId: string;
  planId: string;
  /** Resolved lazily via listBackupPlans() — the `task` event only carries the plan id. Null
   *  while resolving or if the plan was deleted mid-run. */
  planName: string | null;
  progress: TaskProgress | null;
  /** "backup" = bytes transferring; "retention" = the post-backup forget (apply_retention) is
   *  running. Drives the panel subtitle so the finalize step isn't mistaken for a frozen bar. */
  phase: "backup" | "retention";
  /** True once the backup `task` op has reached "finished". A plan with no retention configured
   *  never gets a "forget" op, so this is what tells the plan-lookup effect it's safe to dismiss
   *  the row once it confirms no retention is coming, rather than waiting on an event that will
   *  never arrive. */
  backupFinished: boolean;
}

interface ActivityState {
  /** Background auto-indexing progress; null when auto-indexing is off or fully caught up. */
  indexing: { cached: number; total: number } | null;
  /** The scheduler-triggered backup currently running, if any. Manual/"Run Now" backups never
   *  populate this — derived from `task` events filtered to `origin === "scheduler"`, see
   *  `reduceSchedulerBackup`. */
  activeBackup: ActiveScheduledBackup | null;
  /** The currently-running prune (prune_all_repos or prune_repo), if any — derived from `kind:
   *  "prune"` task events, single-in-flight app-wide (PruneHandle busy flag), see reducePrune.
   *  Lets the Settings "Prune All" modal be dismissed without cancelling the operation, matching
   *  activeIndexBatches' survive-navigation behavior. */
  activePrune: ActivePrune | null;
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
  /** Every in-progress standalone single-snapshot index ("Index Snapshot" / "Index Now"),
   *  across all repos — derived from each per-snapshot manual `task` op (kind "index", targetId
   *  set). Includes a batch's own per-snapshot events too (indistinguishable on the wire — see
   *  ActiveSnapshotIndex's doc comment); ActivityPanel filters out entries whose repoId matches
   *  a currently-running batch so the two rows don't double up for the same repo. */
  activeSnapshotIndexes: ActiveSnapshotIndex[];
  /** Display names for the repoIds among `activeIndexBatches`, `activeSnapshotIndexes`, and
   *  `activeMirrors` (for mirrors, the destination repo), resolved via listRepos() (the event
   *  only carries the id — see the module doc comment above), keyed by repoId. A missing or null
   *  entry means still resolving or the repo can no longer be found (e.g. deleted mid-batch); the
   *  panel falls back to a generic label in that case rather than a raw id. */
  indexBatchRepoNames: Record<string, string | null>;
  /** Every queued or running mirror (RepositoriesPage's "Mirror to another repository"), across
   *  all repos — empty when none are active. Unlike `activePrune` (single-in-flight app-wide),
   *  multiple mirrors can be queued at once, so this is a Map-backed array like
   *  `activeIndexBatches` — see `reduceMirror` and the module doc comment above. */
  activeMirrors: ActiveMirror[];
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

/** A single manually-triggered snapshot index in flight — "Index Snapshot"
 *  (SnapshotsPage context menu) or "Index Now" (SearchPage) — derived from its per-snapshot
 *  `task` op (`kind: "index"`, `origin: "manual"`, `targetId` = snapshot id). This is the
 *  mirror image of `ActiveIndexBatch`: a batch's own per-snapshot progress events share this
 *  exact shape (see reduceSnapshotIndexes), so a repo running an "Index All" batch is
 *  suppressed at render time (ActivityPanel filters by repoId against activeIndexBatches)
 *  rather than distinguished on the wire — there's nothing in the payload that tells them
 *  apart. No progress/cancel affordance: `index_snapshot` has no cancel slot at all (see
 *  browse.rs), so this is a lifecycle-only spinner row, same treatment as `statsRefreshing`. */
export interface ActiveSnapshotIndex {
  operationId: string;
  repoId: string;
  snapshotId: string;
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

/** Pure reducer over `index`-kind task events, isolating standalone manual single-snapshot
 *  index ops ("Index Snapshot" / "Index Now") — the inverse guard of reduceIndexBatches
 *  above: this one only wants events that DO carry a `targetId`. A batch's own per-snapshot
 *  events (index_snapshots_batch, browse.rs) are indistinguishable on the wire from a
 *  standalone `index_snapshot` call, so this reducer tracks both — ActivityPanel is
 *  responsible for filtering out any entry whose repoId matches a currently-running batch
 *  (see its module doc comment) so a batch's per-snapshot progress doesn't also render a
 *  redundant standalone row. `origin: "background"` (the cache-warmer auto-indexer) is
 *  excluded — that's already covered by the `indexing` cached/total row. Returns the same
 *  `state` reference whenever nothing changes so the caller can skip a re-render. Exported
 *  for a unit test (see activity.test.ts). */
export function reduceSnapshotIndexes(
  state: Map<string, ActiveSnapshotIndex>,
  event: TaskEvent
): Map<string, ActiveSnapshotIndex> {
  if (event.kind !== "index" || event.origin !== "manual" || !event.targetId) return state;
  switch (event.phase) {
    case "started": {
      const next = new Map(state);
      next.set(event.operationId, {
        operationId: event.operationId,
        repoId: event.repoId,
        snapshotId: event.targetId,
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
      return state; // "pending"/"progress"/"cancelling" — single-snapshot ops never emit these
  }
}

export interface ActivePrune {
  operationId: string;
  itemsDone: number;
  itemsTotal: number;
  /** Current repo's name from progress.label; null before the first progress tick or for a
   *  single-repo prune (prune_repo emits no progress, so this row stays indeterminate). */
  repoLabel: string | null;
}

/** Pure reducer over `prune`-kind task events. Single nullable (prune is single-in-flight app-wide
 *  via the shared PruneHandle busy flag — see repo.rs), unlike activeIndexBatches' Map. Covers
 *  both prune_all_repos (progress-bearing: itemsDone/itemsTotal/label) and prune_repo
 *  (lifecycle-only — itemsTotal stays 0, rendering as an indeterminate row). operationId guards
 *  keep a stale progress/terminal event from a just-superseded prune from touching a freshly
 *  started one. Returns the same reference when nothing changes so the caller can skip a
 *  re-render. Exported for a unit test (see activity.test.ts). */
export function reducePrune(state: ActivePrune | null, event: TaskEvent): ActivePrune | null {
  if (event.kind !== "prune") return state;
  switch (event.phase) {
    case "started":
      return { operationId: event.operationId, itemsDone: 0, itemsTotal: 0, repoLabel: null };
    case "progress":
      if (!state || state.operationId !== event.operationId) return state;
      return {
        ...state,
        itemsDone: event.progress?.itemsDone ?? state.itemsDone,
        itemsTotal: event.progress?.itemsTotal ?? state.itemsTotal,
        repoLabel: event.progress?.label ?? state.repoLabel,
      };
    case "finished":
    case "failed":
    case "cancelled":
      if (!state || state.operationId !== event.operationId) return state;
      return null;
    default:
      return state; // "pending"/"cancelling" — prune never emits pending; cancelling is local UI state
  }
}

/** A queued or running mirror (RepositoriesPage's "Mirror to another repository") — derived from
 *  its `task` op. `status` starts "queued" on a "pending" event and flips to "running" once
 *  "started" arrives, mirroring `ActiveIndexBatch`'s same two-state shape. No `itemsDone`/
 *  `itemsTotal` — mirror_repo emits no `progress` phase (restic `copy` streams no `--json`
 *  progress the way backup does), so a running row always renders as an indeterminate bar. */
export interface ActiveMirror {
  operationId: string;
  repoId: string;
  status: "queued" | "running";
}

/** Pure reducer over `mirror`-kind task events, one entry per concurrently queued/running mirror
 *  (keyed by operationId — see the module doc comment above). Unlike `reduceIndexBatches`, no
 *  `origin`/`targetId` guard is needed: mirror_repo always emits `origin: "manual"` and never
 *  sets `targetId` (it copies every snapshot, not one), so `kind === "mirror"` alone is enough to
 *  isolate these events — the same single-guard shape `reducePrune` uses. Returns the same `state`
 *  reference whenever nothing changes so the caller can skip a re-render. Exported for a unit
 *  test (see activity.test.ts). */
export function reduceMirror(
  state: Map<string, ActiveMirror>,
  event: TaskEvent
): Map<string, ActiveMirror> {
  if (event.kind !== "mirror") return state;
  switch (event.phase) {
    case "pending": {
      const next = new Map(state);
      next.set(event.operationId, {
        operationId: event.operationId,
        repoId: event.repoId,
        status: "queued",
      });
      return next;
    }
    case "started": {
      const next = new Map(state);
      next.set(event.operationId, {
        operationId: event.operationId,
        repoId: event.repoId,
        status: "running",
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
      return state; // "progress"/"cancelling" — mirror never emits progress; cancelling is local UI state
  }
}

/** Pure reducer over `task` events, isolating the scheduler-triggered backup lifecycle (see the
 *  module doc comment above). Filters to `origin === "scheduler"`; `kind:"backup"` drives the
 *  transfer phase, `kind:"forget"` (matched by `targetId === planId`) drives retention. The
 *  no-retention dismissal isn't handled here — the reducer alone can't know whether a plan has
 *  retention configured, so a `finished` backup with no retention just sits with
 *  `backupFinished: true` until the plan-lookup effect in the provider clears it. Exported for a
 *  unit test (see activity.test.ts). */
export function reduceSchedulerBackup(
  state: ActiveScheduledBackup | null,
  event: TaskEvent
): ActiveScheduledBackup | null {
  if (event.origin !== "scheduler") return state;

  if (event.kind === "backup") {
    switch (event.phase) {
      case "started":
        return {
          runId: event.operationId,
          currentOperationId: event.operationId,
          planId: event.targetId ?? "",
          planName: null,
          progress: null,
          phase: "backup",
          backupFinished: false,
        };
      case "progress":
        if (!state || state.currentOperationId !== event.operationId) return state;
        return { ...state, progress: event.progress ?? state.progress };
      case "finished":
        if (!state || state.currentOperationId !== event.operationId) return state;
        // Stays visible — a "forget" op may still follow (see the no-retention note above).
        return { ...state, backupFinished: true };
      case "failed":
      case "cancelled":
        if (!state || state.currentOperationId !== event.operationId) return state;
        return null;
      default:
        return state; // "pending"/"cancelling" — backup never emits pending; cancelling is local UI state
    }
  }

  if (event.kind === "forget") {
    switch (event.phase) {
      case "started":
        if (!state || !state.backupFinished || event.targetId !== state.planId) return state;
        return { ...state, currentOperationId: event.operationId, phase: "retention" };
      case "finished":
      case "failed":
        if (!state || state.currentOperationId !== event.operationId) return state;
        return null;
      default:
        return state;
    }
  }

  return state;
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
  const [activePrune, setActivePrune] = useState<ActivePrune | null>(null);
  const [upcoming, setUpcoming] = useState<UpcomingBackup[]>([]);
  const [recentLogs, setRecentLogs] = useState<BackupHistoryEntry[]>([]);
  const [statsRefreshing, setStatsRefreshing] = useState<string[]>([]);
  const [statsFailed, setStatsFailed] = useState<string[]>([]);
  const [activeIndexBatches, setActiveIndexBatches] = useState<ActiveIndexBatch[]>([]);
  const [activeSnapshotIndexes, setActiveSnapshotIndexes] = useState<ActiveSnapshotIndex[]>([]);
  const [activeMirrors, setActiveMirrors] = useState<ActiveMirror[]>([]);
  const [indexBatchRepoNames, setIndexBatchRepoNames] = useState<Record<string, string | null>>({});
  const [clockTick, setClockTick] = useState(0);
  const [statsRefreshAllProgress, setStatsRefreshAllProgress] = useState<{ current: number; total: number } | null>(null);
  // statsRefreshing/statsFailed (both deduped repoId arrays) are derived from this on every
  // "task" event via reduceStatsOps.
  const statsOpsRef = useRef<StatsOpsState>(initialStatsOpsState);
  // activeIndexBatches is derived from this on every "task" event via reduceIndexBatches;
  // mirrored into a ref (like statsOpsRef) so the listener can compare by reference and only
  // call setActiveIndexBatches when the set of batches actually changed.
  const indexBatchesRef = useRef<Map<string, ActiveIndexBatch>>(new Map());
  // activeSnapshotIndexes is derived from this on every "task" event via
  // reduceSnapshotIndexes; same compare-by-reference discipline as indexBatchesRef above.
  const snapshotIndexesRef = useRef<Map<string, ActiveSnapshotIndex>>(new Map());
  // activeBackup is derived from this on every "task" event via reduceSchedulerBackup; also
  // written directly by the plan-lookup effect below (planName resolution, no-retention
  // dismissal) so the ref stays the source of truth the next reduce builds on rather than
  // reverting a plan name the reducer itself never sets.
  const schedulerBackupRef = useRef<ActiveScheduledBackup | null>(null);
  // activePrune is derived from this on every "task" event via reducePrune; same
  // compare-by-reference discipline as the refs above.
  const pruneRef = useRef<ActivePrune | null>(null);
  // activeMirrors is derived from this on every "task" event via reduceMirror; same
  // compare-by-reference discipline as indexBatchesRef above (a Map, since multiple mirrors
  // can be queued/running at once — see reduceMirror's doc comment).
  const mirrorsRef = useRef<Map<string, ActiveMirror>>(new Map());

  const refreshIndexing = () => { loadIndexing().then(setIndexing).catch(() => {}); };
  const refreshUpcoming = () => { loadUpcoming().then(setUpcoming).catch(() => {}); };
  const refreshLogs = () => { listBackupHistory().then((h) => setRecentLogs(h.slice(0, 3))).catch(() => {}); };

  useEffect(() => {
    refreshIndexing();
    refreshUpcoming();
    refreshLogs();

    const tickTimer = setInterval(() => setClockTick((t) => t + 1), 60_000);

    const unlistenSnapshotsRefreshed = listen("snapshots:refreshed", refreshIndexing);
    // upcoming is refreshed via the schedules:changed event the scheduler emits after
    // record_schedule_run advances next_run_at — that fires after all plans + retention
    // complete, which is when the next fire time is actually known.
    const unlistenSchedulesChanged = listen("schedules:changed", refreshUpcoming);

    // Fires for every backup, manual or scheduled (snapshot.rs's execute_backup) — unlike the
    // scheduler-only lifecycle activeBackup tracks below, this covers manual runs too, so a
    // manually-run backup doesn't stay missing from Recent Logs until the next scheduled tick.
    const unlistenHistoryUpdated = listen("backup:history-updated", refreshLogs);

    // Subscriber to the unified `task` bus — see module doc comment above. "stats" kind events
    // drive statsRefreshing/statsFailed; "index" kind events (replacing the legacy index:done)
    // trigger a plain refetch, and each batch-level "index" op (no targetId) additionally
    // drives activeIndexBatches; "backup"/"forget" kind events with origin "scheduler" drive
    // activeBackup (replacing the legacy scheduler:* events, retired outright). Every other kind
    // is ignored here (it's still observed by App.tsx's dev-only console.debug effect).
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
      const nextSnapIdx = reduceSnapshotIndexes(snapshotIndexesRef.current, e.payload);
      if (nextSnapIdx !== snapshotIndexesRef.current) {
        snapshotIndexesRef.current = nextSnapIdx;
        setActiveSnapshotIndexes([...nextSnapIdx.values()]);
      }
      const nextSchedulerBackup = reduceSchedulerBackup(schedulerBackupRef.current, e.payload);
      if (nextSchedulerBackup !== schedulerBackupRef.current) {
        schedulerBackupRef.current = nextSchedulerBackup;
        setActiveBackup(nextSchedulerBackup);
      }
      const nextPrune = reducePrune(pruneRef.current, e.payload);
      if (nextPrune !== pruneRef.current) {
        pruneRef.current = nextPrune;
        setActivePrune(nextPrune);
      }
      const nextMirrors = reduceMirror(mirrorsRef.current, e.payload);
      if (nextMirrors !== mirrorsRef.current) {
        mirrorsRef.current = nextMirrors;
        setActiveMirrors([...nextMirrors.values()]);
      }
    });

    return () => {
      clearInterval(tickTimer);
      unlistenSnapshotsRefreshed.then((fn) => fn());
      unlistenSchedulesChanged.then((fn) => fn());
      unlistenHistoryUpdated.then((fn) => fn());
      unlistenTask.then((fn) => fn());
    };
  }, []);

  // The task event only carries repoId, not a display name — resolve the whole set of
  // currently-active batches', standalone snapshot indexes', AND mirrors' repoIds (for mirrors,
  // the destination — see ActiveMirror's doc comment) in one listRepos() call (same by-id lookup
  // loadUpcoming does for plan names above) whenever that set changes. Joined into a stable
  // string key so the effect only re-runs when the actual set of repos changes, not on every
  // progress tick. Falls back to null per id (panel shows a generic label) if a repo can't be
  // found or the lookup fails, including the rare case of a repo being deleted mid-batch/
  // mid-index/mid-mirror.
  const batchRepoIdsKey = [
    ...new Set([
      ...activeIndexBatches.map((b) => b.repoId),
      ...activeSnapshotIndexes.map((s) => s.repoId),
      ...activeMirrors.map((m) => m.repoId),
    ]),
  ].sort().join(",");
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

  // Resolves activeBackup's plan name (the `task` event only carries the plan id — same
  // rationale as indexBatchRepoNames above) via listBackupPlans(), and doubles as the
  // no-retention dismissal: a plan with no keep_* flag set never gets a "forget" op (see
  // scheduler.rs), so reduceSchedulerBackup alone can't know to clear the row once the backup
  // finishes — only this lookup, which sees the plan's actual retention config, can. Keyed on
  // `runId` (stable across the backup->retention transition) plus `backupFinished` so it
  // re-checks once the backup completes, when the no-retention case actually becomes decidable.
  const activeRunId = activeBackup?.runId;
  const activePlanId = activeBackup?.planId;
  const activeBackupFinished = activeBackup?.backupFinished ?? false;
  useEffect(() => {
    if (!activeRunId || !activePlanId) return;
    let cancelled = false;
    listBackupPlans()
      .then((plans) => {
        if (cancelled) return;
        const plan = plans.find((p) => p.id === activePlanId);
        const r = plan?.retention;
        const hasRetention = !!r && (
          r.keepLast != null || r.keepDaily != null || r.keepWeekly != null ||
          r.keepMonthly != null || r.keepYearly != null
        );
        setActiveBackup((prev) => {
          if (!prev || prev.runId !== activeRunId) return prev; // stale — a newer run has since started
          if (prev.backupFinished && prev.phase === "backup" && !hasRetention) {
            schedulerBackupRef.current = null;
            return null;
          }
          const next = { ...prev, planName: plan?.name ?? null };
          schedulerBackupRef.current = next;
          return next;
        });
      })
      .catch(() => {
        if (cancelled) return;
        setActiveBackup((prev) => {
          if (!prev || prev.runId !== activeRunId) return prev;
          const next = { ...prev, planName: null };
          schedulerBackupRef.current = next;
          return next;
        });
      });
    return () => { cancelled = true; };
  }, [activeRunId, activePlanId, activeBackupFinished]);

  return (
    <ActivityContext.Provider
      value={{
        indexing, activeBackup, activePrune, upcoming, recentLogs, statsRefreshing, statsFailed,
        activeIndexBatches, activeSnapshotIndexes, activeMirrors, indexBatchRepoNames, clockTick,
        statsRefreshAllProgress, setStatsRefreshAllProgress,
      }}
    >
      {children}
    </ActivityContext.Provider>
  );
}
