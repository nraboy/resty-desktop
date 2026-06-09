import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { checkRepo, deleteSnapshot, listRepos, listSnapshots, refreshSnapshots, restoreSnapshot, tagSnapshot, unlockRepo } from "../lib/invoke";
import type { CheckResult, Repository, RestoreProgress, Snapshot } from "../lib/types";
import { isRemoteRepo } from "../lib/types";
import Button from "../components/Button";
import Modal from "../components/Modal";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";

function formatDate(iso: string) {
  return new Date(iso).toLocaleString();
}

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export default function SnapshotsPage() {
  const navigate = useNavigate();
  const { repoId } = useParams<{ repoId: string }>();
  const [repo, setRepo] = useState<Repository | null>(null);
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<Snapshot | null>(null);
  const [pruneOnDelete, setPruneOnDelete] = useState(true);
  const [deleting, setDeleting] = useState(false);
  const [tagTarget, setTagTarget] = useState<Snapshot | null>(null);
  const [newTag, setNewTag] = useState("");
  const [tagging, setTagging] = useState(false);
  const [filter, setFilter] = useState("");
  const [unlockConfirm, setUnlockConfirm] = useState(false);
  const [unlocking, setUnlocking] = useState(false);
  const [checking, setChecking] = useState(false);
  const [checkResult, setCheckResult] = useState<CheckResult | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<Snapshot | null>(null);
  const [restoreDir, setRestoreDir] = useState("");
  const [restoring, setRestoring] = useState(false);
  const [restoreDone, setRestoreDone] = useState(false);
  const [restoreProgress, setRestoreProgress] = useState<RestoreProgress | null>(null);
  const restoreUnlistenRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    if (!repoId) return;
    listRepos().then((repos) => {
      const found = repos.find((r) => r.id === repoId) ?? null;
      setRepo(found);
    });
  }, [repoId]);

  const refresh = useCallback(async () => {
    if (!repoId) return;
    setRefreshing(true);
    setError("");
    try {
      const data = await refreshSnapshots(repoId);
      setSnapshots(data.reverse());
    } catch (err: any) {
      setError(String(err));
    } finally {
      setRefreshing(false);
    }
  }, [repoId]);

  const load = useCallback(async () => {
    if (!repoId || !repo) return;
    const willRefresh = !isRemoteRepo(repo.path);
    setLoading(true);
    if (willRefresh) setRefreshing(true);
    try {
      const cached = await listSnapshots(repoId);
      setSnapshots(cached.reverse());
    } finally {
      setLoading(false);
    }
    if (!willRefresh) return;
    refreshSnapshots(repoId)
      .then((data) => setSnapshots(data.reverse()))
      .catch(() => {})
      .finally(() => setRefreshing(false));
  }, [repoId, repo]);

  useEffect(() => {
    load();
  }, [load]);

  const handleDelete = async () => {
    if (!repoId || !deleteTarget) return;
    setDeleting(true);
    try {
      await deleteSnapshot(repoId, deleteTarget.id, pruneOnDelete);
      setDeleteTarget(null);
      await refresh();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setDeleting(false);
    }
  };

  const handleUnlock = async () => {
    if (!repoId) return;
    setUnlocking(true);
    try {
      await unlockRepo(repoId);
      setUnlockConfirm(false);
    } catch (err: any) {
      setError(String(err));
      setUnlockConfirm(false);
    } finally {
      setUnlocking(false);
    }
  };

  const handleCheck = async () => {
    if (!repoId) return;
    setChecking(true);
    setCheckResult(null);
    try {
      const result = await checkRepo(repoId);
      setCheckResult(result);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setChecking(false);
    }
  };

  const handleAddTag = async () => {
    if (!repoId || !tagTarget || !newTag.trim()) return;
    setTagging(true);
    try {
      await tagSnapshot(repoId, tagTarget.id, [newTag.trim()], []);
      setNewTag("");
      setTagTarget(null);
      await refresh();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setTagging(false);
    }
  };

  const handleRemoveTag = useCallback(async (snapshot: Snapshot, tag: string) => {
    if (!repoId) return;
    try {
      await tagSnapshot(repoId, snapshot.id, [], [tag]);
      await refresh();
    } catch (err: any) {
      setError(String(err));
    }
  }, [repoId, refresh]);

  const handlePickRestoreDir = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") setRestoreDir(dir);
  };

  const handleRestore = async () => {
    if (!repoId || !restoreTarget || !restoreDir) return;
    setRestoring(true);
    setRestoreDone(false);
    setRestoreProgress(null);
    const unlisten = await listen<RestoreProgress>("restore:progress", (e) => {
      setRestoreProgress(e.payload);
    });
    restoreUnlistenRef.current = unlisten;
    try {
      await restoreSnapshot(repoId, restoreTarget.id, restoreDir);
      setRestoreDone(true);
    } catch (err: any) {
      setError(String(err));
      setRestoreTarget(null);
    } finally {
      unlisten();
      restoreUnlistenRef.current = null;
      setRestoring(false);
      setRestoreProgress(null);
    }
  };

  const filtered = useMemo(() =>
    filter
      ? snapshots.filter(
          (s) =>
            s.short_id.includes(filter) ||
            s.hostname.toLowerCase().includes(filter.toLowerCase()) ||
            (s.tags ?? []).some((t) => t.toLowerCase().includes(filter.toLowerCase())) ||
            s.paths.some((p) => p.toLowerCase().includes(filter.toLowerCase()))
        )
      : snapshots,
    [snapshots, filter]);

  if (!repoId || (!repo && !loading)) {
    return (
      <EmptyState
        title="Repository not found"
        description="This repository no longer exists."
        action={
          <Button variant="secondary" onClick={() => navigate("/")}>
            Go to Repositories
          </Button>
        }
      />
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <Button variant="ghost" size="sm" onClick={() => navigate("/")}>
            ← Repositories
          </Button>
          <h1 className="text-xl font-semibold text-gray-100 mt-2">Snapshots</h1>
          {repo && <p className="text-sm text-gray-500 mt-0.5">{repo.name}</p>}
        </div>
        <div className="flex items-center gap-3">
          <Input
            placeholder="Filter snapshots…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            className="w-56"
          />
          {refreshing && <span className="text-xs text-gray-500">Updating…</span>}
          <Button variant="secondary" onClick={refresh} loading={refreshing}>
            Refresh
          </Button>
          <Button variant="secondary" onClick={handleCheck} loading={checking}>
            Check
          </Button>
          <Button variant="secondary" onClick={() => setUnlockConfirm(true)}>
            Unlock
          </Button>
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {!loading && !refreshing && filtered.length === 0 ? (
        <EmptyState
          title="No snapshots"
          description="Run a backup to create the first snapshot."
        />
      ) : (
        <div className="rounded-xl border border-gray-800 overflow-hidden">
          <table className="w-full text-sm">
            <thead>
              <tr className="bg-gray-900 border-b border-gray-800 text-left">
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">ID</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Date</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Host</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Paths</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Tags</th>
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-20">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-800">
              {filtered.map((snap) => (
                <tr key={snap.id} className="hover:bg-gray-900/50 transition-colors">
                  <td className="px-4 py-3 font-mono text-blue-400 text-xs">{snap.short_id}</td>
                  <td className="px-4 py-3 text-gray-300 whitespace-nowrap">{formatDate(snap.time)}</td>
                  <td className="px-4 py-3 text-gray-400">{snap.hostname}</td>
                  <td className="px-4 py-3 text-gray-400 max-w-xs">
                    <div className="text-xs text-gray-400 cursor-default" title={snap.paths.join("\n")}>{snap.paths.length} {snap.paths.length === 1 ? "path" : "paths"}</div>
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex flex-wrap gap-1">
                      {(snap.tags ?? []).map((tag) => (
                        <span
                          key={tag}
                          className="inline-flex items-center gap-1 text-xs bg-gray-800 text-gray-300 px-2 py-0.5 rounded-full"
                        >
                          {tag}
                          <button
                            onClick={() => handleRemoveTag(snap, tag)}
                            className="text-gray-500 hover:text-red-400 transition-colors"
                          >
                            ×
                          </button>
                        </span>
                      ))}
                      <button
                        onClick={() => { setTagTarget(snap); setNewTag(""); }}
                        className="text-xs text-gray-600 hover:text-blue-400 transition-colors px-1"
                      >
                        + tag
                      </button>
                    </div>
                  </td>
                  <td className="px-4 py-3">
                    <div className="flex items-center gap-1">
                      <button
                        title="Browse files"
                        onClick={() => navigate(`/snapshots/${repoId}/${snap.id}/browse`, { state: { snapshot: snap } })}
                        className="p-1.5 rounded text-gray-400 hover:text-blue-400 hover:bg-gray-800 transition-colors"
                      >
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                          <path d="M2 6a2 2 0 012-2h5l2 2h5a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
                        </svg>
                      </button>
                      <button
                        title="Restore snapshot"
                        onClick={() => { setRestoreTarget(snap); setRestoreDir(""); setRestoreDone(false); }}
                        className="p-1.5 rounded text-gray-400 hover:text-green-400 hover:bg-gray-800 transition-colors"
                      >
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                          <path d="M10.75 2.75a.75.75 0 00-1.5 0v8.614L6.295 8.235a.75.75 0 10-1.09 1.03l4.25 4.5a.75.75 0 001.09 0l4.25-4.5a.75.75 0 00-1.09-1.03l-2.955 3.129V2.75z" />
                          <path d="M3.5 12.75a.75.75 0 00-1.5 0v2.5A2.75 2.75 0 004.75 18h10.5A2.75 2.75 0 0018 15.25v-2.5a.75.75 0 00-1.5 0v2.5c0 .69-.56 1.25-1.25 1.25H4.75c-.69 0-1.25-.56-1.25-1.25v-2.5z" />
                        </svg>
                      </button>
                      <button
                        title="Delete snapshot"
                        onClick={() => setDeleteTarget(snap)}
                        className="p-1.5 rounded text-gray-600 hover:text-red-400 hover:bg-gray-800 transition-colors"
                      >
                        <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                          <path fillRule="evenodd" d="M8.75 1A2.75 2.75 0 006 3.75v.443c-.795.077-1.584.176-2.365.298a.75.75 0 10.23 1.482l.149-.022.841 10.518A2.75 2.75 0 007.596 19h4.807a2.75 2.75 0 002.742-2.53l.841-10.52.149.023a.75.75 0 00.23-1.482A41.03 41.03 0 0014 4.193v-.443A2.75 2.75 0 0011.25 1h-2.5zM10 4c.84 0 1.673.025 2.5.075V3.75c0-.69-.56-1.25-1.25-1.25h-2.5c-.69 0-1.25.56-1.25 1.25v.325C8.327 4.025 9.16 4 10 4zM8.58 7.72a.75.75 0 00-1.5.06l.3 7.5a.75.75 0 101.5-.06l-.3-7.5zm4.34.06a.75.75 0 10-1.5-.06l-.3 7.5a.75.75 0 101.5.06l.3-7.5z" clipRule="evenodd" />
                        </svg>
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Modal
        title="Delete Snapshot"
        open={deleteTarget !== null}
        onClose={() => setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-4">
          Delete snapshot <span className="font-mono text-blue-400">{deleteTarget?.short_id}</span>?
          This cannot be undone.
        </p>
        <label className="flex items-center gap-2 text-sm text-gray-300 mb-4 cursor-pointer">
          <input
            type="checkbox"
            checked={pruneOnDelete}
            onChange={(e) => setPruneOnDelete(e.target.checked)}
            className="rounded bg-gray-700 border-gray-600"
          />
          Also run <span className="font-mono text-xs">restic prune</span> after forget
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)}>Cancel</Button>
          <Button variant="danger" loading={deleting} onClick={handleDelete}>Delete</Button>
        </div>
      </Modal>

      <Modal
        title="Unlock Repository"
        open={unlockConfirm}
        onClose={() => setUnlockConfirm(false)}
      >
        <p className="text-sm text-gray-300 mb-4">
          Remove all stale locks from this repository. Only do this if you are certain no other
          restic process is currently running against it — unlocking an active operation can corrupt the repository.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setUnlockConfirm(false)}>Cancel</Button>
          <Button variant="danger" loading={unlocking} onClick={handleUnlock}>Unlock</Button>
        </div>
      </Modal>

      <Modal
        title="Repository Check"
        open={checkResult !== null}
        onClose={() => setCheckResult(null)}
      >
        {checkResult && (
          <>
            {checkResult.errors.length === 0 ? (
              <div className={`flex flex-col items-center justify-center py-8 gap-2 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-400"}`}>
                {checkResult.success ? (
                  <>
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-8 h-8 shrink-0">
                      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
                    </svg>
                    No errors found
                  </>
                ) : (
                  <>
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-8 h-8 shrink-0">
                      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                    </svg>
                    Errors found
                  </>
                )}
              </div>
            ) : (
              <>
                <div className={`flex items-center gap-2 mb-4 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-400"}`}>
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                    <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                  </svg>
                  Errors found
                </div>
                <div className="mb-4 space-y-2">
                  {checkResult.errors.map((err, i) => (
                    <div key={i} className="text-xs font-mono bg-red-950/40 border border-red-800 rounded p-2 text-red-300 break-all">
                      {err}
                    </div>
                  ))}
                </div>
              </>
            )}
            <div className="flex items-center justify-between mt-4">
              <span className="text-xs text-gray-500">Completed in {checkResult.duration_seconds.toFixed(1)}s</span>
              <Button variant="secondary" onClick={() => setCheckResult(null)}>Close</Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Add Tag"
        open={tagTarget !== null}
        onClose={() => setTagTarget(null)}
      >
        <p className="text-sm text-gray-400 mb-3">
          Add a tag to snapshot <span className="font-mono text-blue-400">{tagTarget?.short_id}</span>
        </p>
        <div className="flex gap-2">
          <Input
            placeholder="Tag name"
            value={newTag}
            onChange={(e) => setNewTag(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleAddTag()}
            className="flex-1"
          />
          <Button loading={tagging} onClick={handleAddTag}>Add</Button>
        </div>
      </Modal>

      <Modal
        title="Restore Snapshot"
        open={restoreTarget !== null}
        onClose={() => { if (!restoring) { setRestoreTarget(null); setRestoreDone(false); } }}
      >
        {restoreDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Restore complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Snapshot <span className="font-mono text-blue-400">{restoreTarget?.short_id}</span> was restored to{" "}
              <span className="font-mono text-gray-300 break-all">{restoreDir}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setRestoreTarget(null); setRestoreDone(false); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Restore all files from snapshot{" "}
              <span className="font-mono text-blue-400">{restoreTarget?.short_id}</span> to a target directory.
              Existing files in the target will be overwritten.
            </p>
            <div className="flex gap-2 mb-4">
              <div className="flex-1">
                <Input
                  placeholder="Select a target directory…"
                  value={restoreDir}
                  onChange={(e) => setRestoreDir(e.target.value)}
                  className="w-full"
                />
              </div>
              <Button variant="secondary" onClick={handlePickRestoreDir}>Browse</Button>
            </div>
            {restoring && (
              <div className="mb-4">
                <div className="flex justify-between text-xs text-gray-400 mb-1">
                  <span>
                    {restoreProgress
                      ? `${restoreProgress.filesRestored.toLocaleString()} / ${restoreProgress.totalFiles.toLocaleString()} files`
                      : "Starting…"}
                  </span>
                  {restoreProgress && (
                    <span>{formatBytes(restoreProgress.bytesRestored)} / {formatBytes(restoreProgress.totalBytes)}</span>
                  )}
                </div>
                <div className="w-full bg-gray-800 rounded-full h-2">
                  <div
                    className="bg-blue-500 h-2 rounded-full transition-all duration-300"
                    style={{ width: `${Math.round((restoreProgress?.percentDone ?? 0) * 100)}%` }}
                  />
                </div>
              </div>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setRestoreTarget(null)} disabled={restoring}>Cancel</Button>
              <Button onClick={handleRestore} loading={restoring} disabled={!restoreDir}>
                Restore
              </Button>
            </div>
          </>
        )}
      </Modal>
    </div>
  );
}
