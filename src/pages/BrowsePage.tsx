import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getRestorePath, listFiles, listRepos, restorePath, tagSnapshot } from "../lib/invoke";
import type { Snapshot } from "../lib/types";
import type { FileEntry, Repository } from "../lib/types";
import { formatSize, formatDateOnly } from "../lib/format";
import Button from "../components/Button";
import ContextMenu from "../components/ContextMenu";
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
  const locationState = location.state as { snapshot?: Snapshot; initialPath?: string; initialPathStack?: string[]; fromSearch?: boolean } | null;
  const snapshot = locationState?.snapshot;
  const fromSearch = locationState?.fromSearch ?? false;
  const [repo, setRepo] = useState<Repository | null>(null);
  const [entries, setEntries] = useState<FileEntry[]>([]);
  const [currentPath, setCurrentPath] = useState<string | undefined>(locationState?.initialPath);
  const [pathStack, setPathStack] = useState<string[]>(locationState?.initialPathStack ?? []);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [restoreTarget, setRestoreTarget] = useState<FileEntry | null>(null);
  const [targetDir, setTargetDir] = useState("");
  const [defaultTargetDir, setDefaultTargetDir] = useState("");
  const [stripLeadingPath, setStripLeadingPath] = useState(true);
  const [restoring, setRestoring] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; entry: FileEntry } | null>(null);
  const [showHidden, setShowHidden] = useState(false);
  const [tags, setTags] = useState<string[]>(snapshot?.tags ?? []);
  const [showTagModal, setShowTagModal] = useState(false);
  const [newTag, setNewTag] = useState("");
  const [tagging, setTagging] = useState(false);
  const [selectionMode, setSelectionMode] = useState(false);
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(new Set());
  const [multiRestoreOpen, setMultiRestoreOpen] = useState(false);
  const [multiTargetDir, setMultiTargetDir] = useState("");
  const [multiStripLeadingPath, setMultiStripLeadingPath] = useState(true);
  const [multiRestoring, setMultiRestoring] = useState(false);
  const [multiRestoreProgress, setMultiRestoreProgress] = useState<{ current: number; total: number; currentPath: string } | null>(null);
  const [multiRestoreError, setMultiRestoreError] = useState("");

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
    load(locationState?.initialPath);
  // locationState is stable (from router, never changes for this mount)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  const enterDir = useCallback((entry: FileEntry) => {
    setPathStack((s) => [...s, currentPath ?? ""]);
    setSelectedPaths(new Set());
    load(entry.path);
  }, [currentPath, load]);

  const exitSelectionMode = useCallback(() => {
    setSelectionMode(false);
    setSelectedPaths(new Set());
  }, []);

  const toggleSelected = useCallback((path: string) => {
    setSelectedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const visibleEntries = useMemo(() =>
    entries
      .filter((entry) => showHidden || !entry.name.startsWith("."))
      .sort((a, b) => {
        if (a.type === "dir" && b.type !== "dir") return -1;
        if (a.type !== "dir" && b.type === "dir") return 1;
        return a.name.localeCompare(b.name);
      }),
  [entries, showHidden]);

  const allVisibleSelected = visibleEntries.length > 0 && visibleEntries.every((e) => selectedPaths.has(e.path));

  const toggleSelectAll = useCallback(() => {
    if (allVisibleSelected) {
      setSelectedPaths(new Set());
    } else {
      setSelectedPaths(new Set(visibleEntries.map((e) => e.path)));
    }
  }, [allVisibleSelected, visibleEntries]);

  const handleMultiRestore = async () => {
    if (!repoId || !snapshotId || !multiTargetDir) return;
    setMultiRestoring(true);
    setMultiRestoreError("");
    const paths = Array.from(selectedPaths);
    for (let i = 0; i < paths.length; i++) {
      setMultiRestoreProgress({ current: i, total: paths.length, currentPath: paths[i] });
      try {
        await restorePath(repoId, snapshotId, paths[i], multiTargetDir, multiStripLeadingPath);
        setMultiRestoreProgress({ current: i + 1, total: paths.length, currentPath: paths[i] });
      } catch (err: any) {
        setMultiRestoreError(String(err));
        setMultiRestoreProgress(null);
        setMultiRestoring(false);
        return;
      }
    }
    setMultiRestoring(false);
    setMultiRestoreProgress(null);
    setMultiRestoreOpen(false);
    exitSelectionMode();
  };

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
      await restorePath(repoId, snapshotId, restoreTarget.path, targetDir, stripLeadingPath);
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
        <div className="flex items-center gap-3">
          {fromSearch ? (
            <Button variant="ghost" size="sm" onClick={() => navigate(-1)}>
              ← Search
            </Button>
          ) : (
            <Button variant="ghost" size="sm" onClick={() => navigate(`/snapshots/${repoId}`)}>
              ← Snapshots
            </Button>
          )}
        </div>
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
        <button onClick={() => { setPathStack([]); setSelectedPaths(new Set()); load(); }} className="hover:text-gray-200 transition-colors">
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
                onClick={() => { setPathStack(pathStack.slice(0, i)); setSelectedPaths(new Set()); load(p); }}
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
        <div className="ml-auto pl-4 flex items-center gap-4">
          {selectionMode ? (
            <button
              onClick={exitSelectionMode}
              className="text-blue-400 hover:text-blue-300 transition-colors font-medium"
            >
              Cancel select
            </button>
          ) : (
            <button
              onClick={() => setSelectionMode(true)}
              className="hover:text-gray-200 transition-colors"
            >
              Select Multiple
            </button>
          )}
          <div className="w-px h-4 bg-gray-700 flex-shrink-0" />
          <label className="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.target.checked)}
              className="w-4 h-4 accent-blue-500"
            />
            Show hidden files
          </label>
          <div className="w-px h-4 bg-gray-700 flex-shrink-0" />
          <button
            title="Search files in this snapshot"
            onClick={() => navigate(`/snapshots/${repoId}/${snapshotId}/search`, { state: { snapshot, fromBrowse: true, returnPath: currentPath, returnStack: pathStack } })}
            className="hover:text-gray-200 transition-colors"
          >
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
              <path fillRule="evenodd" d="M9 3.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11ZM2 9a7 7 0 1 1 12.452 4.391l3.328 3.329a.75.75 0 1 1-1.06 1.06l-3.329-3.328A7 7 0 0 1 2 9Z" clipRule="evenodd" />
            </svg>
          </button>
        </div>
      </div>

      {selectionMode && selectedPaths.size > 0 && (
        <div className="mb-3 px-4 py-2.5 rounded-lg bg-blue-900/30 border border-blue-700 flex items-center justify-between">
          <span className="text-sm text-blue-300">
            {selectedPaths.size} item{selectedPaths.size !== 1 ? "s" : ""} selected
          </span>
          <Button
            size="sm"
            onClick={() => { setMultiTargetDir(defaultTargetDir); setMultiStripLeadingPath(true); setMultiRestoreError(""); setMultiRestoreProgress(null); setMultiRestoreOpen(true); }}
          >
            Restore selected
          </Button>
        </div>
      )}

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      <div className="rounded-xl border border-gray-800 overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="bg-gray-900 border-b border-gray-800 text-left">
              {selectionMode && (
                <th className="pl-4 py-3 w-10">
                  <input
                    type="checkbox"
                    checked={allVisibleSelected}
                    onChange={toggleSelectAll}
                    className="w-4 h-4 accent-blue-500"
                  />
                </th>
              )}
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Name</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Size</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Modified</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-24">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-800">
            {loading ? (
              <tr>
                <td colSpan={selectionMode ? 5 : 4}>
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
                <tr
                  key={entry.path}
                  className={`hover:bg-gray-900/50 transition-colors ${selectionMode && selectedPaths.has(entry.path) ? "bg-blue-900/10" : ""}`}
                  onContextMenu={(e) => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, entry }); }}
                >
                  {selectionMode && (
                    <td className="pl-4 py-2.5 w-10">
                      <input
                        type="checkbox"
                        checked={selectedPaths.has(entry.path)}
                        onChange={() => toggleSelected(entry.path)}
                        className="w-4 h-4 accent-blue-500"
                      />
                    </td>
                  )}
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
                    {entry.mtime ? formatDateOnly(entry.mtime) : "—"}
                  </td>
                  <td className="px-4 py-2.5">
                    <button
                      title="Restore"
                      onClick={() => { setRestoreTarget(entry); setTargetDir(defaultTargetDir); setStripLeadingPath(true); }}
                      className="p-1.5 rounded text-gray-400 hover:text-green-400 hover:bg-gray-800 transition-colors"
                    >
                      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                        <path d="M10.75 2.75a.75.75 0 00-1.5 0v8.614L6.295 8.235a.75.75 0 10-1.09 1.03l4.25 4.5a.75.75 0 001.09 0l4.25-4.5a.75.75 0 00-1.09-1.03l-2.955 3.129V2.75z" />
                        <path d="M3.5 12.75a.75.75 0 00-1.5 0v2.5A2.75 2.75 0 004.75 18h10.5A2.75 2.75 0 0018 15.25v-2.5a.75.75 0 00-1.5 0v2.5c0 .69-.56 1.25-1.25 1.25H4.75c-.69 0-1.25-.56-1.25-1.25v-2.5z" />
                      </svg>
                    </button>
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

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Restore…",
              onClick: () => { setRestoreTarget(contextMenu.entry); setTargetDir(defaultTargetDir); setStripLeadingPath(true); },
            },
          ]}
        />
      )}

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
        title="Restore Selected"
        open={multiRestoreOpen}
        onClose={() => { if (!multiRestoring) { setMultiRestoreOpen(false); setMultiRestoreProgress(null); setMultiRestoreError(""); } }}
      >
        {multiRestoreProgress ? (
          <div className="py-2">
            <p className="text-sm text-gray-300 mb-3">
              Restoring {multiRestoreProgress.current} of {multiRestoreProgress.total}…
            </p>
            <p className="text-xs font-mono text-gray-500 break-all mb-4">{multiRestoreProgress.currentPath}</p>
            <div className="w-full bg-gray-800 rounded-full h-1.5">
              <div
                className="bg-blue-500 h-1.5 rounded-full transition-all"
                style={{ width: `${(multiRestoreProgress.current / multiRestoreProgress.total) * 100}%` }}
              />
            </div>
          </div>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Restore <span className="text-blue-400">{selectedPaths.size} item{selectedPaths.size !== 1 ? "s" : ""}</span> to a target directory.
            </p>
            {multiRestoreError && (
              <div className="mb-3 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 break-all">
                {multiRestoreError}
              </div>
            )}
            <div className="flex gap-2 mb-3">
              <div className="flex-1">
                <Input
                  placeholder="Select a target directory…"
                  value={multiTargetDir}
                  onChange={(e) => setMultiTargetDir(e.target.value)}
                />
              </div>
              <Button variant="secondary" onClick={async () => {
                const dir = await openDialog({ directory: true, multiple: false });
                if (typeof dir === "string") setMultiTargetDir(dir);
              }}>Browse</Button>
            </div>
            <label className="flex items-center gap-2 cursor-pointer select-none mb-4 text-sm text-gray-400">
              <input
                type="checkbox"
                checked={multiStripLeadingPath}
                onChange={(e) => setMultiStripLeadingPath(e.target.checked)}
                className="w-4 h-4 accent-blue-500"
              />
              Restore files/folders only (skip original path structure)
            </label>
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => { setMultiRestoreOpen(false); setMultiRestoreError(""); }} disabled={multiRestoring}>
                Cancel
              </Button>
              <Button loading={multiRestoring} onClick={handleMultiRestore} disabled={!multiTargetDir}>
                Restore
              </Button>
            </div>
          </>
        )}
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
        <div className="flex gap-2 mb-3">
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
        <label className="flex items-center gap-2 cursor-pointer select-none mb-4 text-sm text-gray-400">
          <input
            type="checkbox"
            checked={stripLeadingPath}
            onChange={(e) => setStripLeadingPath(e.target.checked)}
            className="w-4 h-4 accent-blue-500"
          />
          Restore {restoreTarget?.type === "dir" ? "folder" : "file"} only (skip original path structure)
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setRestoreTarget(null)} disabled={restoring}>Cancel</Button>
          <Button loading={restoring} onClick={handleRestore} disabled={!targetDir}>Restore</Button>
        </div>
      </Modal>
    </div>
  );
}
