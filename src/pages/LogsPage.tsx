import { Fragment, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { checkFullDiskAccess, getBackupWarnings, listBackupHistory } from "../lib/invoke";
import type { FullDiskAccessStatus } from "../lib/invoke";
import { CANCELLED_BACKUP_ERROR } from "../lib/types";
import type { BackupHistoryEntry, BackupWarning } from "../lib/types";
import { formatBytes, formatDate, formatDuration } from "../lib/format";
import { isPermissionWarning } from "../lib/utils";
import Button from "../components/Button";
import EmptyState from "../components/EmptyState";
import BackupWarningsList from "../components/BackupWarningsList";
import { CheckCircleIcon, XCircleIcon, MinusCircleIcon, WarningIcon } from "../components/icons";

const PAGE_SIZE = 10;

export default function LogsPage() {
  const navigate = useNavigate();
  const [entries, setEntries] = useState<BackupHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [error, setError] = useState("");
  // Warning detail per history id, fetched on first expand (the list endpoint omits it).
  // `null` = fetch failed; the row then shows the count with no detail.
  const [warningDetail, setWarningDetail] = useState<Record<string, BackupWarning[] | null>>({});
  // macOS Full Disk Access state, for the hint on permission warnings. Unknown = no hint.
  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);

  const toggleExpanded = (entry: BackupHistoryEntry) => {
    const opening = expanded !== entry.id;
    setExpanded(opening ? entry.id : null);
    if (opening && entry.warningCount > 0 && !(entry.id in warningDetail)) {
      getBackupWarnings(entry.id)
        .then((list) => setWarningDetail((prev) => ({ ...prev, [entry.id]: list })))
        .catch(() => setWarningDetail((prev) => ({ ...prev, [entry.id]: null })));
    }
  };

  const load = async () => {
    setLoading(true);
    setError("");
    try {
      setEntries(await listBackupHistory());
      setPage(0);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);
  useEffect(() => {
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
  }, []);

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-gray-500 text-sm">Loading…</p>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Backup Logs</h1>
          <p className="text-sm text-gray-500 mt-0.5">History of all backup runs.</p>
        </div>
        <Button variant="secondary" onClick={load}>Refresh</Button>
      </div>

      {error && (
        <p className="text-sm text-red-300 mb-4">{error}</p>
      )}

      {entries.length === 0 ? (
        <EmptyState
          title="No backup history"
          description="Backup logs will appear here after you run a backup."
        />
      ) : (() => {
        const totalPages = Math.ceil(entries.length / PAGE_SIZE);
        const pageEntries = entries.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
        return (
          <>
            <div className="rounded-xl border border-gray-800 overflow-x-auto">
              <table className="w-full min-w-[800px] text-sm">
                <thead>
                  <tr className="bg-gray-900 border-b border-gray-800 text-left">
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-6"></th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Date</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Plan</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Repository</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Duration</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">New</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Changed</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Added</th>
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-28">Snapshot</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-gray-800">
                  {pageEntries.map((entry) => {
                    const cancelled = entry.error === CANCELLED_BACKUP_ERROR;
                    const realError = entry.error && !cancelled;
                    const hasWarnings = !entry.error && entry.warningCount > 0;
                    const expandable = realError || hasWarnings;
                    return (
                    <Fragment key={entry.id}>
                      <tr
                        className={`transition-colors ${expandable ? "hover:bg-gray-900/50" : ""}`}
                        onClick={() => expandable && toggleExpanded(entry)}
                      >
                        <td className="px-4 py-3">
                          {cancelled ? (
                            <span title="Cancelled">
                              <MinusCircleIcon className="w-4 h-4 text-gray-500" />
                            </span>
                          ) : entry.error ? (
                            <span title={entry.error}>
                              <XCircleIcon className="w-4 h-4 text-red-300" />
                            </span>
                          ) : hasWarnings ? (
                            <span title={entry.missingSourceCount > 0 ? "Incomplete: source path missing" : `Completed with warnings (${entry.warningCount})`}>
                              <WarningIcon className="w-4 h-4 text-amber-400" />
                            </span>
                          ) : (
                            <CheckCircleIcon className="w-4 h-4 text-green-400" />
                          )}
                        </td>
                        <td className="px-4 py-3 text-gray-300 whitespace-nowrap">{formatDate(entry.startedAt)}</td>
                        <td className="px-4 py-3 text-gray-300">{entry.planName ?? <span className="text-gray-500 italic">Manual</span>}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.repoName ?? entry.repoId}</td>
                        <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatDuration(entry.durationSeconds, true)}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.filesNew.toLocaleString()}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.filesChanged.toLocaleString()}</td>
                        <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatBytes(entry.bytesAdded)}</td>
                        <td className="px-4 py-3 font-mono text-blue-400 text-xs">
                          {entry.snapshotId ? (
                            <button
                              title="Browse files"
                              onClick={(e) => {
                                e.stopPropagation();
                                navigate(`/snapshots/${entry.repoId}/${entry.snapshotId}/browse`);
                              }}
                              className="hover:underline"
                            >
                              {entry.snapshotId.slice(0, 8)}
                            </button>
                          ) : (
                            <span className="text-gray-500">—</span>
                          )}
                        </td>
                      </tr>
                      {expanded === entry.id && realError && (
                        <tr className="bg-red-900/20">
                          <td colSpan={9} className="px-4 py-3">
                            <p className="text-xs font-mono text-red-300 whitespace-pre-wrap break-all">{entry.error}</p>
                          </td>
                        </tr>
                      )}
                      {expanded === entry.id && hasWarnings && (
                        <tr className="bg-amber-900/20">
                          <td colSpan={9} className="px-4 py-3">
                            {entry.id in warningDetail ? (() => {
                              const list = warningDetail[entry.id] ?? [];
                              return (
                                <BackupWarningsList
                                  warnings={list}
                                  warningCount={entry.warningCount}
                                  missingSourceCount={entry.missingSourceCount}
                                  fdaHint={!!fdaStatus?.supported && !fdaStatus.granted && list.some((w) => isPermissionWarning(w.message))}
                                />
                              );
                            })() : (
                              <p className="text-xs text-gray-500">Loading…</p>
                            )}
                          </td>
                        </tr>
                      )}
                    </Fragment>
                    );
                  })}
                </tbody>
              </table>
            </div>
            {totalPages > 1 && (
              <div className="flex items-center justify-between mt-4">
                <p className="text-sm text-gray-500">
                  Page {page + 1} of {totalPages} · {entries.length} total entries
                </p>
                <div className="flex gap-2">
                  <Button variant="secondary" onClick={() => setPage(p => p - 1)} disabled={page === 0}>
                    Previous
                  </Button>
                  <Button variant="secondary" onClick={() => setPage(p => p + 1)} disabled={page >= totalPages - 1}>
                    Next
                  </Button>
                </div>
              </div>
            )}
          </>
        );
      })()}
    </div>
  );
}
