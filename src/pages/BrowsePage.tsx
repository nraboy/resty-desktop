import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getRestorePath, listFiles, listRepos, restorePath, tagSnapshot } from "../lib/invoke";
import type { Snapshot } from "../lib/types";
import type { FileEntry, Repository } from "../lib/types";
import { formatSize } from "../lib/format";
import Button from "../components/Button";
import Modal from "../components/Modal";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";

const FileIcon = ({ type }: { type: string }) => {
  if (type === "dir") {
    return (
      <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
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
  const location = useLocation();
  const snapshot = (location.state as { snapshot?: Snapshot } | null)?.snapshot;
  const [repo, setRepo] = useState<Repository | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState<string | undefined>(undefined);
  const [pathStack, setPathStack] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [restoreTarget, setRestoreTarget] = useState<FileEntry | null>(null);
  const [targetDir, setTargetDir] = useState("");
  const [defaultTargetDir, setDefaultTargetDir] = useState("");
  const [restoring, setRestoring] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const [tags, setTags] = useState<string[]>(snapshot?.tags ?? []);
  const [showTagModal, setShowTagModal] = useState(false);
  const [newTag, setNewTag] = useState("");
  const [tagging, setTagging] = useState(false);

  useEffect(() => {
    getRestorePath().then(setDefaultTargetDir).catch(() => {});
  }, []);

  useEffect(() => {
    if (!repoId) return;
    listRepos().then((repos) => {
      setRepo(repos.find((r) => r.id === repoId) ?? null);
    });
  }, [repoId]);

  const load = useCallback(
    async (path?: string) => {
      if (!repoId || !snapshotId) return;
      setLoading(true);
      setError("");
      setCurrentPath(path);
      try {
        const data = await listFiles(repoId, snapshotId, path);
        setEntries(data);
      } catch (err: any) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    },
    [repoId, snapshotId]
  );

  useEffect(() => {
    load();
  }, [load]);

  const enterDir = useCallback((entry: FileEntry) => {
    setPathStack((s) => [...s, currentPath ?? ""]);
    load(entry.path);
  }, [currentPath, load]);

  const visibleEntries = useMemo(() =>
    entries
      .filter((entry) => showHidden || !entry.name.startsWith("."))
      .sort((a, b) => {
        if (a.type === "dir" && b.type !== "dir") return -1;
        if (a.type !== "dir" && b.type === "dir") return 1;
        return a.name.localeCompare(b.name);
      }),
  [entries, showHidden]);

  const handleAddTag = async () => {
    if (!repoId || !snapshotId || !newTag.trim()) return;
    setTagging(true);
    try {
      await tagSnapshot(repoId, snapshotId, [newTag.trim()], []);
      setTags((prev) => [...prev, newTag.trim()]);
      setNewTag("");
      setShowTagModal(false);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setTagging(false);
    }
  };

  const handleRemoveTag = async (tag: string) => {
    if (!repoId || !snapshotId) return;
    try {
      await tagSnapshot(repoId, snapshotId, [], [tag]);
      setTags((prev) => prev.filter((t) => t !== tag));
    } catch (err: any) {
      setError(String(err));
    }
  };

  const handlePickTargetDir = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") setTargetDir(dir);
  };

  const handleRestore = async () => {
    if (!repoId || !snapshotId || !restoreTarget || !targetDir) return;
    setRestoring(true);
    try {
      await restorePath(repoId, snapshotId, restoreTarget.path, targetDir);
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
      <div className="mb-6">
        <Button variant="ghost" size="sm" onClick={() => navigate(`/snapshots/${repoId}`)}>
          ← Snapshots
        </Button>
        <h1 className="text-xl font-semibold text-gray-100 mt-3">Browse Snapshot</h1>
        {repo && (
          <div className="flex items-center gap-2 mt-0.5 flex-wrap">
            <p className="text-sm text-gray-400">{repo.name}</p>
            <div className="flex items-center gap-1 flex-wrap">
              {tags.map((tag) => (
                <span key={tag} className="flex items-center gap-1 pl-1.5 pr-0.5 py-0.5 text-xs rounded bg-gray-800 text-gray-400 border border-gray-700">
                  {tag}
                  <button
                    onClick={() => handleRemoveTag(tag)}
                    className="text-gray-600 hover:text-red-300 transition-colors leading-none"
                    title="Remove tag"
                  >
                    ×
                  </button>
                </span>
              ))}
              <button
                onClick={() => { setNewTag(""); setShowTagModal(true); }}
                className="px-1.5 py-0.5 text-xs rounded border border-dashed border-gray-700 text-gray-600 hover:text-gray-400 hover:border-gray-500 transition-colors"
                title="Add tag"
              >
                + tag
              </button>
            </div>
          </div>
        )}
        <p className="text-xs text-gray-600 font-mono mt-0.5">{snapshotId}</p>
      </div>

      <div className="flex items-center gap-1 mb-4 text-sm text-gray-400 justify-between">
        <button onClick={() => { setPathStack([]); load(); }} className="hover:text-gray-200 transition-colors">
          root
        </button>
        {pathStack.map((p, i) => {
          if (!p) return null;
          return (
            // eslint-disable-next-line react/no-array-index-key
            <span key={i} className="contents">
              <span className="text-gray-700">/</span>
              <button
                className="hover:text-gray-200 transition-colors"
                onClick={() => { setPathStack(pathStack.slice(0, i)); load(p); }}
              >
                {p.split("/").pop() || "/"}
              </button>
            </span>
          );
        })}
        {currentPath && (
          <>
            <span className="text-gray-700">/</span>
            <span className="text-gray-300">{currentPath.split("/").pop()}</span>
          </>
        )}
        <label className="flex items-center gap-2 cursor-pointer select-none ml-auto pl-4">
          <input
            type="checkbox"
            checked={showHidden}
            onChange={(e) => setShowHidden(e.target.checked)}
            className="w-4 h-4 accent-blue-500"
          />
          Show hidden files
        </label>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

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
            {loading ? (
              <tr>
                <td colSpan={4}>
                  <div className="flex items-center justify-center py-20 text-gray-500">
                    <svg className="animate-spin w-6 h-6 mr-2" viewBox="0 0 24 24" fill="none">
                      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
                    </svg>
                    Loading…
                  </div>
                </td>
              </tr>
            ) : (
              visibleEntries.map((entry) => (
                <tr key={entry.path} className="hover:bg-gray-900/50 transition-colors">
                  <td className="px-4 py-2.5">
                    <div className="flex items-center gap-2">
                      <FileIcon type={entry.type} />
                      {entry.type === "dir" ? (
                        <button
                          onClick={() => enterDir(entry)}
                          className="text-gray-200 hover:text-gray-50 transition-colors text-left"
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
                      onClick={() => { setRestoreTarget(entry); setTargetDir(defaultTargetDir); }}
                    >
                      Restore
                    </Button>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
        {!loading && entries.length === 0 && (
          <div className="py-10 text-center text-gray-500 text-sm">Empty directory</div>
        )}
      </div>

      <Modal
        title="Add Tag"
        open={showTagModal}
        onClose={() => setShowTagModal(false)}
      >
        <p className="text-sm text-gray-300 mb-4">
          Add a tag to snapshot <span className="font-mono text-blue-400">{snapshotId?.slice(0, 8)}</span>
        </p>
        <Input
          label="Tag"
          value={newTag}
          onChange={(e) => setNewTag(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") handleAddTag(); }}
          placeholder="e.g. weekly"
        />
        <div className="flex justify-end gap-2 mt-4">
          <Button variant="secondary" onClick={() => setShowTagModal(false)}>Cancel</Button>
          <Button loading={tagging} onClick={handleAddTag}>Add</Button>
        </div>
      </Modal>

      <Modal
        title="Restore"
        open={restoreTarget !== null}
        onClose={() => { if (!restoring) setRestoreTarget(null); }}
      >
        <p className="text-sm text-gray-300 mb-4">
          Restore <span className="font-mono text-blue-400 text-xs break-all">{restoreTarget?.path}</span> to a target directory.
          Only files that conflict with the restored content will be overwritten; other files in the target are left untouched.
        </p>
        <div className="flex gap-2 mb-4">
          <div className="flex-1">
            <Input
              placeholder="Select a target directory…"
              value={targetDir}
              onChange={(e) => setTargetDir(e.target.value)}
              className="w-full"
            />
          </div>
          <Button variant="secondary" onClick={handlePickTargetDir}>Browse</Button>
        </div>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setRestoreTarget(null)} disabled={restoring}>Cancel</Button>
          <Button loading={restoring} onClick={handleRestore} disabled={!targetDir}>Restore</Button>
        </div>
      </Modal>
    </div>
  );
}
