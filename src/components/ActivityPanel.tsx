// Right-side activity overlay surfacing background activity the user has no other
// visibility into: auto-indexing progress, scheduler-triggered backups, in-flight repo
// stats refreshes, and the manual "Index All" batch (ACTIVE TASKS), the next couple of due
// schedules (UPCOMING TASKS), and the last couple of backup runs (RECENT LOGS).
// Restore/copy/mirror/manual backup/prune already have their own progress modals and are
// intentionally excluded — see src/lib/activity.tsx. "Index All" is the one exception: its
// modal (RepoSearchPage) is explicitly dismissible while the batch keeps running in the
// background, so — unlike those other modals — it needs a way to stay visible and cancellable
// after the modal closes.
// The stats row is this app's first consumer of the unified `task` event bus rather than a
// per-operation legacy feed (stats never had one) — it's lifecycle-only (no progress bar,
// since a single `restic stats` call has no measurable progress); RepositoriesPage owns the
// actual per-row numbers via its own `task` listener re-reading the DB cache. The "Index All"
// rows are a later consumer of the same bus, and the first to read `progress` (itemsDone/
// itemsTotal) rather than treat the bus purely as a lifecycle signal — see activity.tsx's
// reduceIndexBatches. There can be more than one such row at once: each batch gets its own
// cancel flag on the backend (IndexHandle::batches, cache.rs), so concurrent "Index All" runs
// (e.g. for different repos) are tracked and stoppable independently rather than colliding.
//
// Layout: a slim 24px rail (with an active-dot indicator) always sits in the flex row as a
// normal sibling, so it never changes the width available to routed page content. Clicking it
// opens a `fixed` drawer that slides in over the content (no reflow, no scrim) — dismissed via
// the chevron button or a click outside the drawer. The drawer is always mounted and animated
// with a transform so it slides both in and out; it's just off-screen + non-interactive when
// closed.
//
// Open/closed state is self-contained (toggled here, always closed on launch) so mounting
// this once in App.tsx doesn't require every page to grow a toolbar toggle button.
import { useEffect, useRef, useState } from "react";
import { useActivity } from "../lib/activity";
import { cancelBackup, cancelIndexBatch } from "../lib/invoke";
import { CANCELLED_BACKUP_ERROR } from "../lib/types";
import { formatBytes, formatRelative } from "../lib/format";
import Spinner from "./Spinner";

function ProgressBar({ percent, colorClass = "bg-blue-500" }: { percent: number; colorClass?: string }) {
  return (
    <div className="w-full bg-gray-800 rounded-full h-1.5 overflow-hidden">
      <div
        className={`${colorClass} h-1.5 rounded-full transition-all duration-500`}
        style={{ width: `${Math.min(100, Math.max(0, percent)).toFixed(1)}%` }}
      />
    </div>
  );
}

function SectionHeading({ children }: { children: string }) {
  return <h3 className="text-xs font-semibold text-gray-500 tracking-wider uppercase px-4 pt-4 pb-2">{children}</h3>;
}

function EmptyRow({ children }: { children: string }) {
  return <p className="px-4 pb-3 text-xs text-gray-600 italic">{children}</p>;
}

function SuccessIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-green-400 flex-shrink-0">
      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
    </svg>
  );
}

function ErrorIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-red-300 flex-shrink-0">
      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
    </svg>
  );
}

function CancelledIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-gray-500 flex-shrink-0">
      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM7 9a1 1 0 000 2h6a1 1 0 100-2H7z" clipRule="evenodd" />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
      <rect x="5" y="5" width="10" height="10" rx="2" />
    </svg>
  );
}

export default function ActivityPanel() {
  const { indexing, activeBackup, upcoming, recentLogs, statsRefreshing, activeIndexBatches, indexBatchRepoNames } = useActivity();
  const [open, setOpen] = useState(false);
  const panelRef = useRef<HTMLElement>(null);

  const hasActive = indexing != null || activeBackup != null || statsRefreshing.length > 0 || activeIndexBatches.length > 0;

  // Cancel affordance for a scheduler-triggered backup — cancelBackup() already kills
  // whatever's in BackupHandle.child regardless of whether it was started manually or by the
  // scheduler (unchanged since v0.3.0); the only thing missing was a button to call it from
  // here. Resets automatically once activeBackup clears (scheduler:backup-finished fires
  // regardless of outcome — success, failure, or this very cancel), so it's ready again the
  // next time a scheduled backup runs.
  const [stoppingScheduled, setStoppingScheduled] = useState(false);
  useEffect(() => {
    if (!activeBackup) setStoppingScheduled(false);
  }, [activeBackup]);

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

  useEffect(() => {
    if (!open) return;
    const handleMouseDown = (e: MouseEvent) => {
      if (panelRef.current && !panelRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, [open]);

  return (
    <>
      <button
        onClick={() => setOpen(true)}
        title="Show activity"
        className="flex-shrink-0 w-6 h-full bg-gray-900 border-l border-gray-800 hover:bg-gray-800 transition-colors flex items-center justify-center relative"
      >
        <svg className="w-4 h-4 text-gray-500" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M15 19l-7-7 7-7" />
        </svg>
        {hasActive && (
          <span className="absolute top-3 w-1.5 h-1.5 rounded-full bg-blue-500" aria-label="Activity in progress" />
        )}
      </button>

      <aside
        ref={panelRef}
        className={`fixed inset-y-0 right-0 w-80 z-40 bg-gray-900 border-l border-gray-800 flex flex-col overflow-y-auto shadow-xl transition-transform duration-200 ${
          open ? "translate-x-0" : "translate-x-full pointer-events-none"
        }`}
      >
        <div className="px-4 py-4 border-b border-gray-800 flex items-center justify-between flex-shrink-0">
          <h2 className="text-sm font-bold text-gray-50 tracking-tight">Task Activity</h2>
          <button
            onClick={() => setOpen(false)}
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
              {activeIndexBatches.map((batch) => {
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
              {activeBackup && (
                <div className="space-y-2">
                  <div className="flex items-center justify-between gap-2">
                    <p className="text-sm text-gray-200 truncate" title={`${activeBackup.scheduleName} — ${activeBackup.planName}`}>
                      {activeBackup.planName} <span className="text-gray-500">· {activeBackup.scheduleName}</span>
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
                      ? `${activeBackup.progress.filesDone.toLocaleString()} / ${activeBackup.progress.totalFiles.toLocaleString()} files`
                      : "Starting…"}
                  </p>
                </div>
              )}
              {statsRefreshing.length > 0 && (
                <div className="flex items-center gap-2">
                  <Spinner className="w-3.5 h-3.5 flex-shrink-0" />
                  <p className="text-sm text-gray-200">
                    Refreshing stats — {statsRefreshing.length} {statsRefreshing.length === 1 ? "repository" : "repositories"}
                  </p>
                </div>
              )}
            </div>
          </div>

          <div className="border-b border-gray-800 pb-1">
            <SectionHeading>Upcoming Tasks</SectionHeading>
            {upcoming.length === 0 && <EmptyRow>No enabled schedules due.</EmptyRow>}
            <div className="space-y-2 px-4 pb-3">
              {upcoming.map((u) => (
                <div key={u.scheduleId} className="text-sm">
                  <p className="text-gray-200 truncate" title={u.scheduleName}>{u.scheduleName}</p>
                  <p
                    className="text-xs text-gray-500 truncate"
                    title={`${u.planNames.join(", ") || "No plans"} · ${formatRelative(u.nextRunAt)}`}
                  >
                    {u.planNames.join(", ") || "No plans"} · {formatRelative(u.nextRunAt)}
                  </p>
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
                    {cancelled ? <CancelledIcon /> : entry.error ? <ErrorIcon /> : <SuccessIcon />}
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
    </>
  );
}
