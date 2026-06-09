import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { deleteSnapshot, listRepos, listSnapshots, refreshSnapshots, tagSnapshot } from "../lib/invoke";
import type { Repository, Snapshot } from "../lib/types";
import { isRemoteRepo } from "../lib/types";
import Button from "../components/Button";
import Modal from "../components/Modal";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";

function formatDate(iso: string) {
  return new Date(iso).toLocaleString();
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
                <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-800">
              {filtered.map((snap) => (
                <tr key={snap.id} className="hover:bg-gray-900/50 transition-colors">
                  <td className="px-4 py-3 font-mono text-blue-400 text-xs">{snap.short_id}</td>
                  <td className="px-4 py-3 text-gray-300 whitespace-nowrap">{formatDate(snap.time)}</td>
                  <td className="px-4 py-3 text-gray-400">{snap.hostname}</td>
                  <td className="px-4 py-3 text-gray-400 max-w-xs">
                    <div className="truncate text-xs font-mono">{snap.paths.join(", ")}</div>
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
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => navigate(`/snapshots/${repoId}/${snap.id}/browse`)}
                      >
                        Browse
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        className="text-red-500 hover:text-red-400"
                        onClick={() => setDeleteTarget(snap)}
                      >
                        Delete
                      </Button>
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
    </div>
  );
}
