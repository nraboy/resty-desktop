import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { listFiles, listRepos, restorePath } from "../lib/invoke";
import type { FileEntry, Repository } from "../lib/types";
import Button from "../components/Button";
import Modal from "../components/Modal";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";

function formatSize(bytes?: number) {
  if (!bytes) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
  return `${(bytes / 1073741824).toFixed(2)} GB`;
}

const FileIcon = ({ type }: { type: string }) => {
  if (type === "dir") {
    return (
      <svg className="w-4 h-4 text-yellow-400 flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
        <path d="M2 6a2 2 0 012-2h4l2 2h6a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
      </svg>
    );
  }
  return (
    <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
        d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
    </svg>
  );
};

export default function BrowsePage() {
  const { repoId, snapshotId } = useParams<{ repoId: string; snapshotId: string }>();
  const navigate = useNavigate();
  const [repo, setRepo] = useState<Repository | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState<string | undefined>(undefined);
  const [pathStack, setPathStack] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [restoreTarget, setRestoreTarget] = useState<FileEntry | null>(null);
  const [targetDir, setTargetDir] = useState("/tmp/restic-restore");
  const [restoring, setRestoring] = useState(false);
  const [showHidden, setShowHidden] = useState(false);

  useEffect(() => {
    if (!repoId) return;
    listRepos().then((repos) => {
      setRepo(repos.find((r) => r.id === repoId) ?? null);
    });
  }, [repoId]);

  const load = useCallback(
    async (path?: string) => {
      if (!repo || !snapshotId) return;
      setLoading(true);
      setError("");
      try {
        const data = await listFiles(repo, snapshotId, path);
        setEntries(data);
        setCurrentPath(path);
      } catch (err: any) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    },
    [repo, snapshotId]
  );

  useEffect(() => {
    load();
  }, [load]);

  const enterDir = (entry: FileEntry) => {
    setPathStack((s) => [...s, currentPath ?? ""]);
    load(entry.path);
  };

  const goBack = () => {
    const prev = pathStack[pathStack.length - 1];
    setPathStack((s) => s.slice(0, -1));
    load(prev || undefined);
  };

  const handleRestore = async () => {
    if (!repo || !snapshotId || !restoreTarget) return;
    setRestoring(true);
    try {
      await restorePath(repo, snapshotId, restoreTarget.path, targetDir);
      setRestoreTarget(null);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setRestoring(false);
    }
  };

  if (!snapshotId || (!repo && !repoId)) {
    return (
      <EmptyState
        title="Snapshot not found"
        action={<Button variant="secondary" onClick={() => navigate("/")}>Go to Repositories</Button>}
      />
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center gap-3 mb-6">
        <Button variant="ghost" size="sm" onClick={() => navigate(`/snapshots/${repoId}`)}>
          ← Snapshots
        </Button>
        <div className="flex-1">
          <h1 className="text-xl font-semibold text-gray-100">Browse Snapshot</h1>
          <p className="text-sm text-gray-500 font-mono mt-0.5">{snapshotId}</p>
        </div>
      </div>

      {/* Toolbar */}
      <div className="flex items-center justify-end mb-4">
        <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={showHidden}
            onChange={(e) => setShowHidden(e.target.checked)}
            className="w-4 h-4 accent-blue-500"
          />
          Show hidden files
        </label>
      </div>

      {/* Breadcrumb */}
      <div className="flex items-center gap-1 mb-4 text-sm text-gray-400">
        <button onClick={() => { setPathStack([]); load(); }} className="hover:text-gray-200 transition-colors">
          /
        </button>
        {pathStack.filter(Boolean).map((p, i) => (
          // eslint-disable-next-line react/no-array-index-key
          <span key={i} className="contents">
            <span className="text-gray-700">/</span>
            <span className="text-gray-500">{p.split("/").pop() || "/"}</span>
          </span>
        ))}
        {currentPath && (
          <>
            <span className="text-gray-700">/</span>
            <span className="text-gray-300">{currentPath.split("/").pop()}</span>
          </>
        )}
      </div>

      {pathStack.length > 0 && (
        <button
          onClick={goBack}
          className="flex items-center gap-2 px-3 py-2 text-sm text-gray-400 hover:text-gray-200 transition-colors mb-2"
        >
          <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 19l-7-7 7-7" />
          </svg>
          ..
        </button>
      )}

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {loading ? (
        <div className="flex items-center justify-center py-20 text-gray-500">
          <svg className="animate-spin w-6 h-6 mr-2" viewBox="0 0 24 24" fill="none">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
          </svg>
          Loading…
        </div>
      ) : (
        <div className="rounded-xl border border-gray-800 overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="bg-gray-900 border-b border-gray-800 text-left">
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Name</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Size</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Modified</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-24">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-800">
              {entries.filter((entry) => showHidden || !entry.name.startsWith(".")).map((entry) => (
                <tr key={entry.path} className="hover:bg-gray-900/50 transition-colors">
                  <td className="px-4 py-2.5">
                    <div className="flex items-center gap-2">
                      <FileIcon type={entry.type} />
                      {entry.type === "dir" ? (
                        <button
                          onClick={() => enterDir(entry)}
                          className="text-gray-200 hover:text-white transition-colors text-left"
                        >
                          {entry.name}
                        </button>
                      ) : (
                        <span className="text-gray-300">{entry.name}</span>
                      )}
                    </div>
                  </td>
                  <td className="px-4 py-2.5 text-gray-500 text-xs">
                    {entry.type === "dir" ? "—" : formatSize(entry.size)}
                  </td>
                  <td className="px-4 py-2.5 text-gray-500 text-xs">
                    {entry.mtime ? new Date(entry.mtime).toLocaleDateString() : "—"}
                  </td>
                  <td className="px-4 py-2.5">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => { setRestoreTarget(entry); setTargetDir("/tmp/restic-restore"); }}
                    >
                      Restore
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {entries.length === 0 && (
            <div className="py-10 text-center text-gray-500 text-sm">Empty directory</div>
          )}
        </div>
      )}

      <Modal
        title="Restore"
        open={restoreTarget !== null}
        onClose={() => setRestoreTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-4">
          Restore <span className="font-mono text-blue-400 text-xs break-all">{restoreTarget?.path}</span> to:
        </p>
        <Input
          label="Target directory"
          value={targetDir}
          onChange={(e) => setTargetDir(e.target.value)}
          placeholder="/tmp/restic-restore"
        />
        <div className="flex justify-end gap-2 mt-4">
          <Button variant="secondary" onClick={() => setRestoreTarget(null)}>Cancel</Button>
          <Button loading={restoring} onClick={handleRestore}>Restore</Button>
        </div>
      </Modal>
    </div>
  );
}
