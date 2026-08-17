// Right-side activity overlay surfacing background activity the user has no other
// visibility into: auto-indexing progress, scheduler-triggered backups, in-flight repo
// stats refreshes, the manual "Index All" batch, "Prune All Repositories", and mirror runs
// (ACTIVE TASKS), the next couple of due schedules (UPCOMING TASKS), and the last couple of
// backup runs (RECENT LOGS). Restore/copy/manual backup still have their own blocking progress
// modals and are intentionally excluded — see src/lib/activity.tsx. "Index All", prune-all, and
// mirror are the exceptions: their modals (RepoSearchPage; SettingsPage's "Prune All
// Repositories"; RepositoriesPage's "Mirror Repository") are explicitly dismissible while the
// operation keeps running in the background, so — unlike those other modals — they need a way
// to stay visible and cancellable after the modal closes.
// The stats row is this app's first consumer of the unified `task` event bus rather than a
// per-operation legacy feed (stats never had one) — it's lifecycle-only (an indeterminate
// ProgressBar, since a single `restic stats` call has no measurable progress); RepositoriesPage owns the
// actual per-row numbers via its own `task` listener re-reading the DB cache. When
// RepositoriesPage's "Refresh Stats" button is running, its batch progress (statsRefreshAllProgress,
// a plain done/total counter the page pushes into ActivityProvider directly — there's no
// backend batch command for this the way index_snapshots_batch is for "Index All", it's just
// a JS loop over the single-repo command) takes over this row instead of the generic
// statsRefreshing count, which would otherwise always read "1 repository" the whole run since
// repos are refreshed one at a time, not in parallel. The "Index All"
// rows are a later consumer of the same bus, and the first to read `progress` (itemsDone/
// itemsTotal) rather than treat the bus purely as a lifecycle signal — see activity.tsx's
// reduceIndexBatches. There can be more than one such row at once: each batch gets its own
// cancel flag on the backend (IndexHandle::batches, cache.rs), so concurrent "Index All" runs
// (e.g. for different repos) are tracked and stoppable independently rather than colliding.
// Only one batch actually runs at a time (IndexHandle::batch_turn) — the rest sit in the
// "Up Next" section below Active Tasks with status "queued" until they're promoted to
// "running" (a "started" task event); Stop works immediately on a queued batch too, it
// doesn't wait for its turn. The scheduler-backup row (activeBackup) is a bus consumer too —
// see activity.tsx's reduceSchedulerBackup — and shows plan name only (the bus carries no
// schedule name, unlike the legacy scheduler:* events it replaced). Mirror rows (activeMirrors)
// are the newest consumer, modeled directly on "Index All": multiple mirrors can be queued at
// once (each with its own cancel flag on MirrorHandle::mirrors, cache.rs), so — like index
// batches — only one actually runs at a time and the rest sit in "Up Next" with status "queued"
// until promoted to "running". Unlike index batches, mirror never emits `progress` (restic
// `copy` has nothing to report incrementally), so a running mirror row is always an
// indeterminate ProgressBar, never a determinate X-of-N bar.
//
// Standalone single-snapshot indexing ("Index Snapshot" on SnapshotsPage, "Index Now" on
// SearchPage) gets its own lifecycle-only row (an indeterminate ProgressBar, no measurable
// progress) per in-flight index (activeSnapshotIndexes, see activity.tsx's
// reduceSnapshotIndexes) — same rationale as "Index All": their modal/inline UI's Close button
// doesn't cancel anything, so this is what stays visible once it's dismissed. No Stop button:
// `index_snapshot` has no cancel path at all (unlike the batch, which does). A batch's own
// per-snapshot events are indistinguishable on the wire from a standalone call, so entries whose
// repo already has a running batch are filtered out here to avoid a duplicate row.
//
// Every "Active Tasks" row uses the same ProgressBar component (determinate when real
// itemsDone/itemsTotal or percentDone is available, indeterminate otherwise) rather than mixing
// in a spinner+text treatment for the lifecycle-only rows — kept visually consistent on
// purpose, see ProgressBar.tsx.
//
// Layout: the Sidebar's footer status strip is the sole affordance that opens/closes this panel
// (there is no right-edge rail). It's a `fixed inset-y-0 right-0` drawer that slides in/out with
// a transform (no reflow, no scrim) — dismissed via the header chevron or a click outside the
// drawer. The drawer is always mounted and animated with a transform so it slides both in and
// out; it's just off-screen + non-interactive when closed.
//
// Open/closed state is owned by App.tsx (passed as props, always closed on launch) so the
// Sidebar's status strip toggles the same drawer — locking unmounts the unlocked branch and
// resets it to closed.
import { useEffect, useRef, useState } from "react";
import { useActivity, activeTaskCount, standaloneSnapshotIndexes } from "../lib/activity";
import { cancelBackup, cancelIndexBatch, cancelMirror, cancelPrune, stopCleanup } from "../lib/invoke";
import { CANCELLED_BACKUP_ERROR } from "../lib/types";
import { formatBytes, formatRelative, isOverdue } from "../lib/format";
import ProgressBar from "./ProgressBar";
import Tooltip from "./Tooltip";
import { CheckCircleIcon, XCircleIcon, MinusCircleIcon } from "./icons";

function SectionHeading({ children }: { children: string }) {
  return <h3 className="text-xs font-semibold text-gray-500 tracking-wider uppercase px-4 pt-4 pb-2">{children}</h3>;
}

function EmptyRow({ children }: { children: string }) {
  return <p className="px-4 pb-3 text-xs text-gray-500 italic">{children}</p>;
}

function StopIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
      <rect x="5" y="5" width="10" height="10" rx="2" />
    </svg>
  );
}

interface ActivityPanelProps {
  /** Whether the drawer is shown. Owned by App.tsx and toggled by the Sidebar's footer status
   *  strip; always closed on launch. */
  open: boolean;
  /** Header chevron + outside-click. Must be referentially stable (App passes useCallback'd
   *  handlers) — the outside-click effect below depends on it. */
  onClose: () => void;
}

export default function ActivityPanel({ open, onClose }: ActivityPanelProps) {
  const {
    indexing, activeBackup, activePrune, activeCleanup, upcoming, recentLogs, statsRefreshing,
    activeIndexBatches, activeSnapshotIndexes, activeMirrors, indexBatchRepoNames,
    statsRefreshAllProgress,
  } = useActivity();
  const panelRef = useRef<HTMLElement>(null);

  // A batch is "queued" (waiting its turn on the backend's batch_turn mutex — see
  // IndexHandle::batch_turn) until it wins the turn and its task event flips to "started",
  // at which point reduceIndexBatches promotes it to "running" and it moves from the "Up
  // Next" section below into this Active Tasks row set.
  const runningIndexBatches = activeIndexBatches.filter((b) => b.status === "running");
  const queuedIndexBatches = activeIndexBatches.filter((b) => b.status === "queued");

  // A batch's own per-snapshot progress events are indistinguishable on the wire from a
  // standalone "Index Snapshot"/"Index Now" call (see ActiveSnapshotIndex's doc comment in
  // activity.tsx) — so suppress any standalone entry whose repo already has a batch (running or
  // queued), rather than rendering a redundant row alongside the batch's own bar. Shared with
  // the Sidebar status strip via the helper so the two can't drift.
  const standaloneIndexes = standaloneSnapshotIndexes(activeIndexBatches, activeSnapshotIndexes);

  // Same "queued" vs "running" split as index batches above — a mirror is "queued" until it
  // wins its turn on the backend's MirrorHandle::turn mutex and its task event flips to
  // "started" (see reduceMirror).
  const runningMirrors = activeMirrors.filter((m) => m.status === "running");
  const queuedMirrors = activeMirrors.filter((m) => m.status === "queued");

  // Shared computation (activity.tsx) — the Sidebar's status strip and this panel's empty-state
  // derive from the same counter, so they can't disagree. Includes queued batches/mirrors.
  const hasActive = activeTaskCount({
    indexing, activeBackup, activePrune, activeCleanup, statsRefreshing, activeIndexBatches,
    activeSnapshotIndexes, activeMirrors, statsRefreshAllProgress,
  }) > 0;

  // Cancel affordance for a scheduler-triggered backup — cancelBackup() already kills
  // whatever's in BackupHandle.child regardless of whether it was started manually or by the
  // scheduler (unchanged since v0.3.0); the only thing missing was a button to call it from
  // here. Resets automatically once activeBackup clears (its underlying `task` op reaches a
  // terminal phase regardless of outcome — success, failure, or this very cancel — see
  // reduceSchedulerBackup), so it's ready again the next time a scheduled backup runs.
  const [stoppingScheduled, setStoppingScheduled] = useState(false);
  useEffect(() => {
    if (!activeBackup) setStoppingScheduled(false);
  }, [activeBackup]);

  // Same pattern, for the prune row below — resets once activePrune clears (its `task` op
  // reached a terminal phase: finished, failed, or this very cancel).
  const [stoppingPrune, setStoppingPrune] = useState(false);
  useEffect(() => {
    if (!activePrune) setStoppingPrune(false);
  }, [activePrune]);

  // Same pattern, for the cleanup row below — resets once activeCleanup clears (its `task` op
  // reached a terminal phase: finished, failed, or this very cancel).
  const [stoppingCleanup, setStoppingCleanup] = useState(false);
  useEffect(() => {
    if (!activeCleanup) setStoppingCleanup(false);
  }, [activeCleanup]);

  // Same pattern as stoppingScheduled above, generalized to a set since multiple "Index All"
  // batches can be stopping independently at once. cancel_index_batch(operationId) takes effect
  // between snapshots (see browse.rs), so an id stays in this set for however long that batch's
  // in-flight snapshot takes to finish; it's pruned once that batch's terminal task event lands
  // (finished/failed/cancelled — see reduceIndexBatches) and the entry disappears from
  // activeIndexBatches.
  const [stoppingBatchIds, setStoppingBatchIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    const liveIds = new Set(activeIndexBatches.map((b) => b.operationId));
    setStoppingBatchIds((prev) => {
      const next = new Set([...prev].filter((id) => liveIds.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [activeIndexBatches]);

  // Same pattern as stoppingBatchIds above, for mirrors — a Set since multiple mirrors can be
  // queued/running (and stopping) at once, unlike prune's single boolean.
  const [stoppingMirrorIds, setStoppingMirrorIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    const liveIds = new Set(activeMirrors.map((m) => m.operationId));
    setStoppingMirrorIds((prev) => {
      const next = new Set([...prev].filter((id) => liveIds.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [activeMirrors]);

  useEffect(() => {
    if (!open) return;
    const handleMouseDown = (e: MouseEvent) => {
      // The Sidebar's Activity toggle owns its own clicks — without this guard, its mousedown
      // would close the drawer here and its subsequent click would toggle it right back open.
      const target = e.target as Element | null;
      if (target?.closest?.("[data-activity-toggle]")) return;
      if (panelRef.current && !panelRef.current.contains(target)) {
        onClose();
      }
    };
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [open, onClose]);

  return (
    <aside
      ref={panelRef}
      className={`fixed inset-y-0 right-0 w-80 z-40 bg-gray-900 border-l border-gray-800 flex flex-col overflow-y-auto shadow-xl transition-transform duration-200 ${
        open ? "translate-x-0" : "translate-x-full pointer-events-none"
      }`}
    >
      <div className="px-4 py-4 border-b border-gray-800 flex items-center justify-between flex-shrink-0">
        <h2 className="text-sm font-bold text-gray-50 tracking-tight">Task Activity</h2>
        <button
          onClick={onClose}
          title="Hide activity"
          className="text-gray-500 hover:text-gray-300 transition-colors"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M9 5l7 7-7 7" />
          </svg>
        </button>
      </div>

      <div className="flex-1">
        <div className="border-b border-gray-800 pb-1">
          <SectionHeading>Active Tasks</SectionHeading>
          {!hasActive && <EmptyRow>Nothing running in the background right now.</EmptyRow>}
          <div className="space-y-3 px-4 pb-3">
            {indexing && (
              <div className="space-y-2">
                <p className="text-sm text-gray-200">Indexing snapshots</p>
                <ProgressBar percent={(indexing.cached / Math.max(1, indexing.total)) * 100} />
                <p className="text-xs text-gray-500">{indexing.cached.toLocaleString()} / {indexing.total.toLocaleString()} indexed</p>
              </div>
            )}
            {runningIndexBatches.map((batch) => {
              const repoName = indexBatchRepoNames[batch.repoId];
              const stopping = stoppingBatchIds.has(batch.operationId);
              return (
                <div key={batch.operationId} className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <p className="text-sm text-gray-200 truncate" title={repoName ?? undefined}>
                      Indexing snapshots{repoName ? ` — ${repoName}` : ""}
                    </p>
                    <button
                      onClick={async () => {
                        setStoppingBatchIds((prev) => new Set(prev).add(batch.operationId));
                        try {
                          await cancelIndexBatch(batch.operationId);
                        } catch {
                          // The cancel call itself failed (e.g. a transient IPC error) — the
                          // batch is still running untouched, so roll back the optimistic
                          // "Stopping…" state rather than leaving Stop stuck disabled with no
                          // way to retry.
                          setStoppingBatchIds((prev) => {
                            const next = new Set(prev);
                            next.delete(batch.operationId);
                            return next;
                          });
                        }
                      }}
                      disabled={stopping}
                      title="Stop"
                      aria-label="Stop"
                      className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                    >
                      <StopIcon />
                    </button>
                  </div>
                  <ProgressBar percent={(batch.itemsDone / Math.max(1, batch.itemsTotal)) * 100} />
                  <p className="text-xs text-gray-500">
                    {stopping ? "Stopping…" : `${batch.itemsDone} / ${batch.itemsTotal} snapshots`}
                  </p>
                </div>
              );
            })}
            {standaloneIndexes.map((s) => {
              const repoName = indexBatchRepoNames[s.repoId];
              const shortId = s.snapshotId.slice(0, 8);
              return (
                <div key={s.operationId} className="space-y-2">
                  <p className="text-sm text-gray-200 truncate" title={repoName ?? undefined}>
                    Indexing snapshot <span className="font-mono">{shortId}</span>
                    {repoName ? ` — ${repoName}` : ""}
                  </p>
                  <ProgressBar indeterminate />
                </div>
              );
            })}
            {activeBackup && (
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-sm text-gray-200 truncate" title={activeBackup.planName ?? undefined}>
                    {activeBackup.planName ?? "Scheduled backup"}
                  </p>
                  {activeBackup.phase === "backup" && (
                    <button
                      onClick={async () => {
                        setStoppingScheduled(true);
                        try { await cancelBackup(); } catch {}
                      }}
                      disabled={stoppingScheduled}
                      title="Stop"
                      aria-label="Stop"
                      className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                    >
                      <StopIcon />
                    </button>
                  )}
                </div>
                <ProgressBar percent={(activeBackup.progress?.percentDone ?? 0) * 100} />
                <p className="text-xs text-gray-500">
                  {activeBackup.phase === "retention"
                    ? "Applying retention rules…"
                    : stoppingScheduled
                    ? "Stopping…"
                    : activeBackup.progress
                    ? `${(activeBackup.progress.itemsDone ?? 0).toLocaleString()} / ${(activeBackup.progress.itemsTotal ?? 0).toLocaleString()} files`
                    : "Starting…"}
                </p>
              </div>
            )}
            {activePrune && (
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-sm text-gray-200 truncate" title={activePrune.repoLabel ?? undefined}>
                    Pruning repositories
                  </p>
                  <button
                    onClick={async () => {
                      setStoppingPrune(true);
                      try {
                        await cancelPrune();
                      } catch {
                        // Same rollback rationale as the other Stop buttons in this panel — the
                        // cancel call itself failed, the prune is still running untouched, so
                        // don't leave Stop stuck disabled with no way to retry.
                        setStoppingPrune(false);
                      }
                    }}
                    disabled={stoppingPrune}
                    title="Stop"
                    aria-label="Stop"
                    className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                  >
                    <StopIcon />
                  </button>
                </div>
                <ProgressBar
                  indeterminate={activePrune.itemsTotal === 0}
                  percent={activePrune.itemsTotal > 0 ? (activePrune.itemsDone / activePrune.itemsTotal) * 100 : 0}
                />
                <p className="text-xs text-gray-500">
                  {stoppingPrune
                    ? "Stopping…"
                    : activePrune.itemsTotal > 0
                    ? `${activePrune.itemsDone} / ${activePrune.itemsTotal} repos`
                    : "Pruning…"}
                </p>
              </div>
            )}
            {activeCleanup && (
              <div className="space-y-2">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-sm text-gray-200 truncate">Cleaning up cache</p>
                  <button
                    onClick={async () => {
                      setStoppingCleanup(true);
                      try {
                        await stopCleanup();
                      } catch {
                        // Same rollback rationale as the other Stop buttons in this panel — the
                        // cancel call itself failed, cleanup is still running untouched, so
                        // don't leave Stop stuck disabled with no way to retry.
                        setStoppingCleanup(false);
                      }
                    }}
                    disabled={stoppingCleanup}
                    title="Stop"
                    aria-label="Stop"
                    className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                  >
                    <StopIcon />
                  </button>
                </div>
                <ProgressBar
                  percent={
                    activeCleanup.itemsTotal > 0
                      ? Math.min(100, (activeCleanup.itemsDone / activeCleanup.itemsTotal) * 100)
                      : 0
                  }
                />
                <p className="text-xs text-gray-500">
                  {stoppingCleanup
                    ? "Stopping…"
                    : `${activeCleanup.itemsDone.toLocaleString()} orphaned entries removed`}
                </p>
              </div>
            )}
            {runningMirrors.map((mirror) => {
              const repoName = indexBatchRepoNames[mirror.repoId];
              const stopping = stoppingMirrorIds.has(mirror.operationId);
              return (
                <div key={mirror.operationId} className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <p className="text-sm text-gray-200 truncate" title={repoName ?? undefined}>
                      Mirroring{repoName ? ` to ${repoName}` : ""}
                    </p>
                    <button
                      onClick={async () => {
                        setStoppingMirrorIds((prev) => new Set(prev).add(mirror.operationId));
                        try {
                          await cancelMirror(mirror.operationId);
                        } catch {
                          // The cancel call itself failed (e.g. a transient IPC error) — the
                          // mirror is still running untouched, so roll back the optimistic
                          // "Stopping…" state rather than leaving Stop stuck disabled with no
                          // way to retry.
                          setStoppingMirrorIds((prev) => {
                            const next = new Set(prev);
                            next.delete(mirror.operationId);
                            return next;
                          });
                        }
                      }}
                      disabled={stopping}
                      title="Stop"
                      aria-label="Stop"
                      className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                    >
                      <StopIcon />
                    </button>
                  </div>
                  <ProgressBar indeterminate />
                  <p className="text-xs text-gray-500">{stopping ? "Stopping…" : "Mirroring…"}</p>
                </div>
              );
            })}
            {statsRefreshAllProgress ? (
              // RepositoriesPage's "Refresh Stats" button — shows real batch progress
              // (current/total, 0-indexed completed-so-far — see statsRefreshAllProgress's
              // doc comment in activity.tsx, same convention as SnapshotsPage's
              // multiDeleteProgress/multiCopyProgress) rather than the generic
              // statsRefreshing row below, which would otherwise always read
              // "1 repository" throughout the whole run (it refreshes repos one at a
              // time, not in parallel — see handleRefreshAll's doc comment). Text shows
              // `current + 1` ("working on repo N") matching SnapshotsPage's own
              // in-progress phrasing; the bar uses raw `current` (0% at start, 100% only
              // once every repo has actually finished).
              <div className="space-y-2">
                <p className="text-sm text-gray-200">
                  Refreshing stats — {statsRefreshAllProgress.current + 1} of {statsRefreshAllProgress.total} repositories
                </p>
                <ProgressBar
                  percent={(statsRefreshAllProgress.current / Math.max(1, statsRefreshAllProgress.total)) * 100}
                />
              </div>
            ) : statsRefreshing.length > 0 && (
              <div className="space-y-2">
                <p className="text-sm text-gray-200">
                  Refreshing stats — {statsRefreshing.length} {statsRefreshing.length === 1 ? "repository" : "repositories"}
                </p>
                <ProgressBar indeterminate />
              </div>
            )}
          </div>
        </div>

        {(queuedIndexBatches.length > 0 || queuedMirrors.length > 0) && (
          <div className="border-b border-gray-800 pb-1">
            <SectionHeading>Up Next</SectionHeading>
            <div className="space-y-2 px-4 pb-3">
              {queuedIndexBatches.map((batch) => {
                const repoName = indexBatchRepoNames[batch.repoId];
                const stopping = stoppingBatchIds.has(batch.operationId);
                return (
                  <div key={batch.operationId} className="flex items-center justify-between gap-2">
                    <p className="text-sm text-gray-400 truncate" title={repoName ?? undefined}>
                      Indexing snapshots{repoName ? ` — ${repoName}` : ""}{" "}
                      <span className="text-xs text-gray-400">· {stopping ? "Stopping…" : "Queued"}</span>
                    </p>
                    <button
                      onClick={async () => {
                        setStoppingBatchIds((prev) => new Set(prev).add(batch.operationId));
                        try {
                          await cancelIndexBatch(batch.operationId);
                        } catch {
                          // Same rollback rationale as the Active Tasks Stop button above.
                          setStoppingBatchIds((prev) => {
                            const next = new Set(prev);
                            next.delete(batch.operationId);
                            return next;
                          });
                        }
                      }}
                      disabled={stopping}
                      title="Stop"
                      aria-label="Stop"
                      className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                    >
                      <StopIcon />
                    </button>
                  </div>
                );
              })}
              {queuedMirrors.map((mirror) => {
                const repoName = indexBatchRepoNames[mirror.repoId];
                const stopping = stoppingMirrorIds.has(mirror.operationId);
                return (
                  <div key={mirror.operationId} className="flex items-center justify-between gap-2">
                    <p className="text-sm text-gray-400 truncate" title={repoName ?? undefined}>
                      Mirroring{repoName ? ` to ${repoName}` : ""}{" "}
                      <span className="text-xs text-gray-400">· {stopping ? "Stopping…" : "Queued"}</span>
                    </p>
                    <button
                      onClick={async () => {
                        setStoppingMirrorIds((prev) => new Set(prev).add(mirror.operationId));
                        try {
                          await cancelMirror(mirror.operationId);
                        } catch {
                          // Same rollback rationale as the Active Tasks Stop button above.
                          setStoppingMirrorIds((prev) => {
                            const next = new Set(prev);
                            next.delete(mirror.operationId);
                            return next;
                          });
                        }
                      }}
                      disabled={stopping}
                      title="Stop"
                      aria-label="Stop"
                      className="text-red-300 hover:text-red-200 flex-shrink-0 disabled:opacity-50"
                    >
                      <StopIcon />
                    </button>
                  </div>
                );
              })}
            </div>
          </div>
        )}

        <div className="border-b border-gray-800 pb-1">
          <SectionHeading>Upcoming Tasks</SectionHeading>
          {upcoming.length === 0 && <EmptyRow>No enabled schedules due.</EmptyRow>}
          <div className="space-y-2 px-4 pb-3">
            {upcoming.map((u) => (
              <div key={u.scheduleId} className="text-sm">
                <p className="text-gray-200 truncate" title={u.scheduleName}>{u.scheduleName}</p>
                <Tooltip
                  content={
                    <div>
                      {u.planNames.length === 0 ? (
                        "No plans"
                      ) : (
                        u.planNames.map((name) => (
                          <div key={name} className="truncate" title={name}>{name}</div>
                        ))
                      )}
                    </div>
                  }
                >
                  <p className="text-xs text-gray-500 truncate cursor-help underline decoration-dotted underline-offset-2">
                    {u.planNames.join(", ") || "No plans"} · {isOverdue(u.nextRunAt) ? "Running soon" : formatRelative(u.nextRunAt)}
                  </p>
                </Tooltip>
              </div>
            ))}
          </div>
        </div>

        <div>
          <SectionHeading>Recent Logs</SectionHeading>
          {recentLogs.length === 0 && <EmptyRow>No backups have run yet.</EmptyRow>}
          <div className="space-y-2 px-4 pb-4">
            {recentLogs.map((entry) => {
              const cancelled = entry.error === CANCELLED_BACKUP_ERROR;
              return (
              <div key={entry.id} className="space-y-0.5">
                <div className="flex items-center gap-2">
                  {cancelled ? (
                    <MinusCircleIcon className="w-4 h-4 text-gray-500 flex-shrink-0" />
                  ) : entry.error ? (
                    <XCircleIcon className="w-4 h-4 text-red-300 flex-shrink-0" />
                  ) : (
                    <CheckCircleIcon className="w-4 h-4 text-green-400 flex-shrink-0" />
                  )}
                  <p className="text-sm text-gray-200 truncate min-w-0">
                    {entry.planName ?? "Manual"} <span className="text-xs text-gray-500">· {formatBytes(entry.bytesAdded)}</span>
                  </p>
                </div>
                <p className="text-xs text-gray-500 truncate pl-6" title={cancelled ? undefined : entry.error}>
                  {cancelled ? `Cancelled, ${formatRelative(entry.startedAt)}` : entry.error ? `Failed, ${formatRelative(entry.startedAt)}` : `Completed, ${formatRelative(entry.startedAt)}`}
                </p>
              </div>
              );
            })}
          </div>
        </div>
      </div>
    </aside>
  );
}
