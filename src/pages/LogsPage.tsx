import { Fragment, useEffect, useState } from "react";
import { listBackupHistory } from "../lib/invoke";
import type { BackupHistoryEntry } from "../lib/types";
import Button from "../components/Button";
import EmptyState from "../components/EmptyState";

function formatDate(ts: number): string {
  return new Date(ts * 1000).toLocaleString();
}

function formatDuration(secs: number): string {
  if (secs < 60) return `${secs.toFixed(1)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.floor(secs % 60)}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export default function LogsPage() {
  const [entries, setEntries] = useState<BackupHistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [expanded, setExpanded] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      setEntries(await listBackupHistory());
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
      ) : (
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
              {entries.map((entry) => (
                <Fragment key={entry.id}>
                  <tr
                    className={`transition-colors ${entry.error ? "cursor-pointer hover:bg-gray-900/50" : ""}`}
                    onClick={() => entry.error && setExpanded(expanded === entry.id ? null : entry.id)}
                  >
                    <td className="px-4 py-3">
                      {entry.error ? (
                        <span title={entry.error}>
                          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-red-400">
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
                    <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatDuration(entry.durationSeconds)}</td>
                    <td className="px-4 py-3 text-gray-400">{entry.filesNew.toLocaleString()}</td>
                    <td className="px-4 py-3 text-gray-400">{entry.filesChanged.toLocaleString()}</td>
                    <td className="px-4 py-3 text-gray-400 whitespace-nowrap">{formatBytes(entry.bytesAdded)}</td>
                    <td className="px-4 py-3 font-mono text-blue-400 text-xs">
                      {entry.snapshotId ? entry.snapshotId.slice(0, 8) : <span className="text-gray-600">—</span>}
                    </td>
                  </tr>
                  {expanded === entry.id && entry.error && (
                    <tr className="bg-red-950/20">
                      <td colSpan={9} className="px-4 py-3">
                        <p className="text-xs font-mono text-red-300 whitespace-pre-wrap">{entry.error}</p>
                      </td>
                    </tr>
                  )}
                </Fragment>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
