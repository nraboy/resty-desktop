// Collapsible right-side drawer surfacing background activity the user has no other
// visibility into: auto-indexing progress and scheduler-triggered backups (ACTIVE TASKS),
// the next couple of due schedules (UPCOMING TASKS), and the last couple of backup runs
// (RECENT LOGS). Restore/copy/mirror/manual backup/prune already have their own progress
// modals and are intentionally excluded — see src/lib/activity.tsx.
//
// Open/closed state is self-contained (toggled here, always closed on launch) so mounting
// this once in App.tsx doesn't require every page to grow a toolbar toggle button.
import { useState } from "react";
import { useActivity } from "../lib/activity";
import { formatBytes, formatRelative } from "../lib/format";

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

export default function ActivityPanel() {
  const { indexing, activeBackup, upcoming, recentLogs } = useActivity();
  const [open, setOpen] = useState(false);

  const hasActive = indexing != null || activeBackup != null;

  if (!open) {
    return (
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
    );
  }

  return (
    <aside className="w-64 flex-shrink-0 bg-gray-900 border-l border-gray-800 flex flex-col h-full overflow-y-auto">
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
            {activeBackup && (
              <div className="space-y-2">
                <p className="text-sm text-gray-200 truncate" title={`${activeBackup.scheduleName} — ${activeBackup.planName}`}>
                  {activeBackup.planName} <span className="text-gray-500">· {activeBackup.scheduleName}</span>
                </p>
                <ProgressBar percent={(activeBackup.progress?.percentDone ?? 0) * 100} />
                <p className="text-xs text-gray-500">
                  {activeBackup.progress
                    ? `${activeBackup.progress.filesDone.toLocaleString()} / ${activeBackup.progress.totalFiles.toLocaleString()} files`
                    : "Starting…"}
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
                <p className="text-gray-200 truncate">{u.scheduleName}</p>
                <p className="text-xs text-gray-500 truncate">
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
            {recentLogs.map((entry) => (
              <div key={entry.id} className="space-y-0.5">
                <div className="flex items-center gap-2">
                  {entry.error ? <ErrorIcon /> : <SuccessIcon />}
                  <p className="text-sm text-gray-200 truncate min-w-0">
                    {entry.planName ?? "Manual"} <span className="text-xs text-gray-500">· {formatBytes(entry.bytesAdded)}</span>
                  </p>
                </div>
                <p className="text-xs text-gray-500 truncate pl-6" title={entry.error}>
                  {entry.error ? `Failed, ${formatRelative(entry.startedAt)}` : `Completed, ${formatRelative(entry.startedAt)}`}
                </p>
              </div>
            ))}
          </div>
        </div>
      </div>
    </aside>
  );
}
