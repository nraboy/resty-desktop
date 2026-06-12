import { Fragment, useEffect, useState } from "react";
import { listBackupHistory } from "../lib/invoke";
import type { BackupHistoryEntry } from "../lib/types";
import { formatBytes, formatDate, formatDuration } from "../lib/format";
import Button from "../components/Button";
import EmptyState from "../components/EmptyState";

const PAGE_SIZE = 10;

export default function LogsPage() {
  const [entries, setEntries] = useState<BackupHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [page, setPage] = useState(0);

  const load = async () => {
    setLoading(true);
    try {
      setEntries(await listBackupHistory());
      setPage(0);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);

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
            <div className="rounded-xl border border-gray-800 overflow-hidden">
              <table className="w-full text-sm">
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
                  {pageEntries.map((entry) => (
                    <Fragment key={entry.id}>
                      <tr
                        className={`transition-colors ${entry.error ? "cursor-pointer hover:bg-gray-900/50" : ""}`}
                        onClick={() => entry.error && setExpanded(expanded === entry.id ? null : entry.id)}
                      >
                        <td className="px-4 py-3">
                          {entry.error ? (
                            <span title={entry.error}>
                              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-red-300">
                                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                              </svg>
                            </span>
                          ) : (
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-green-400">
                              <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
                            </svg>
                          )}
                        </td>
                        <td className="px-4 py-3 text-gray-300 whitespace-nowrap">{formatDate(entry.startedAt)}</td>
                        <td className="px-4 py-3 text-gray-300">{entry.planName ?? <span className="text-gray-600 italic">Manual</span>}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.repoName ?? entry.repoId}</td>
                        <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatDuration(entry.durationSeconds, true)}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.filesNew.toLocaleString()}</td>
                        <td className="px-4 py-3 text-gray-400">{entry.filesChanged.toLocaleString()}</td>
                        <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatBytes(entry.bytesAdded)}</td>
                        <td className="px-4 py-3 font-mono text-blue-400 text-xs">
                          {entry.snapshotId ? entry.snapshotId.slice(0, 8) : <span className="text-gray-600">—</span>}
                        </td>
                      </tr>
                      {expanded === entry.id && entry.error && (
                        <tr className="bg-red-900/20">
                          <td colSpan={9} className="px-4 py-3">
                            <p className="text-xs font-mono text-red-300 whitespace-pre-wrap break-all">{entry.error}</p>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  ))}
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
