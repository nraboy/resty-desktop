import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { diffSnapshots, getRestorePath, restorePath as restorePathInvoke } from "../lib/invoke";
import type { DiffEntry, DiffResult, Snapshot } from "../lib/types";
import Button from "../components/Button";
import EmptyState from "../components/EmptyState";
import ContextMenu from "../components/ContextMenu";
import Modal from "../components/Modal";
import Input from "../components/Input";
import Spinner from "../components/Spinner";

type DiffChange = "added" | "removed" | "modified" | "mixed";

interface TreeNode {
  name: string;
  fullPath: string;
  isDir: boolean;
  change: DiffChange;
}

function toSegments(path: string): string[] {
  return path.replace(/^\//, "").split("/").filter(Boolean);
}

function computeChildren(entries: DiffEntry[], currentPath: string): TreeNode[] {
  const currentSegments = currentPath ? currentPath.split("/").filter(Boolean) : [];
  const depth = currentSegments.length;

  type Parsed = { entry: DiffEntry; segs: string[] };
  const groups = new Map<string, Parsed[]>();

  for (const entry of entries) {
    const segs = toSegments(entry.path);
    if (segs.length <= depth) continue;
    if (!currentSegments.every((seg, i) => segs[i] === seg)) continue;
    const segment = segs[depth];
    if (!groups.has(segment)) groups.set(segment, []);
    groups.get(segment)!.push({ entry, segs });
  }

  const nodes: TreeNode[] = [];
  for (const [name, parsed] of groups) {
    const fullPath = currentPath + "/" + name;
    const nextDepth = depth + 1;
    const isDir = parsed.some((p) => p.segs.length > nextDepth);
    let change: DiffChange;
    if (!isDir) {
      change = parsed[0].entry.change as DiffChange;
    } else {
      const types = new Set(parsed.map((p) => p.entry.change));
      change = types.size === 1 ? ([...types][0] as DiffChange) : "mixed";
    }
    nodes.push({ name, fullPath, isDir, change });
  }

  return nodes.sort((a, b) => {
    if (a.isDir && !b.isDir) return -1;
    if (!a.isDir && b.isDir) return 1;
    return a.name.localeCompare(b.name);
  });
}

const CHANGE_STYLES: Record<DiffChange, { color: string; icon: string; label: string }> = {
  added:    { color: "text-green-400", icon: "+", label: "added"    },
  removed:  { color: "text-red-400",   icon: "−", label: "removed"  },
  modified: { color: "text-amber-400", icon: "~", label: "modified" },
  mixed:    { color: "text-gray-400",  icon: "±", label: "mixed"    },
};

const DirIcon = () => (
  <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
    <path d="M2 6a2 2 0 012-2h4l2 2h6a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
  </svg>
);

const FileIcon = () => (
  <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
      d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
  </svg>
);

export default function DiffPage() {
  const { repoId, snapshotA, snapshotB } = useParams<{ repoId: string; snapshotA: string; snapshotB: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const navState = location.state as { snapshotA?: Snapshot; snapshotB?: Snapshot } | null;

  const [result, setResult] = useState<DiffResult | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [currentPath, setCurrentPath] = useState("");
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; node: TreeNode } | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<{ path: string; snapshotId: string; isDir: boolean } | null>(null);
  const [targetDir, setTargetDir] = useState("");
  const [defaultTargetDir, setDefaultTargetDir] = useState("");
  const [stripLeadingPath, setStripLeadingPath] = useState(true);
  const [restoring, setRestoring] = useState(false);
  const [restoreError, setRestoreError] = useState("");

  useEffect(() => {
    getRestorePath().then(setDefaultTargetDir).catch(() => {});
  }, []);

  useEffect(() => {
    if (!repoId || !snapshotA || !snapshotB) return;
    setLoading(true);
    setError("");
    diffSnapshots(repoId, snapshotA, snapshotB)
      .then(setResult)
      .catch((err) => setError(String(err)))
      .finally(() => setLoading(false));
  }, [repoId, snapshotA, snapshotB]);

  const children = useMemo(
    () => (result ? computeChildren(result.entries, currentPath) : []),
    [result, currentPath]
  );

  const handleRestore = async () => {
    if (!repoId || !restoreTarget || !targetDir) return;
    setRestoring(true);
    setRestoreError("");
    try {
      await restorePathInvoke(repoId, restoreTarget.snapshotId, restoreTarget.path, targetDir, stripLeadingPath);
      setRestoreTarget(null);
    } catch (err: any) {
      setRestoreError(String(err));
    } finally {
      setRestoring(false);
    }
  };

  const openRestoreModal = (node: TreeNode) => {
    // "removed" files only exist in snapshotA (older); everything else restores from snapshotB (newer)
    const snapId = node.change === "removed" ? (snapshotA ?? "") : (snapshotB ?? "");
    setRestoreTarget({ path: node.fullPath, snapshotId: snapId, isDir: node.isDir });
    setTargetDir(defaultTargetDir);
    setStripLeadingPath(true);
    setRestoreError("");
  };

  const handlePickTargetDir = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") setTargetDir(dir);
  };

  const shortA = navState?.snapshotA?.short_id ?? snapshotA?.slice(0, 8) ?? "";
  const shortB = navState?.snapshotB?.short_id ?? snapshotB?.slice(0, 8) ?? "";

  const breadcrumbParts = currentPath ? currentPath.split("/").filter(Boolean) : [];
  const totalChanges = result ? result.totalAdded + result.totalRemoved + result.totalModified : 0;

  return (
    <div className="p-6">
      <div className="mb-6">
        <Button variant="ghost" size="sm" onClick={() => navigate(`/snapshots/${repoId}`)}>
          ← Snapshots
        </Button>
        <h1 className="text-xl font-semibold text-gray-100 mt-3">Snapshot Diff</h1>
        <p className="text-sm text-gray-500 mt-0.5 font-mono">
          {shortA} → {shortB}
        </p>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {loading ? (
        <div className="flex items-center justify-center py-20 text-gray-500 text-sm">
          <Spinner className="w-6 h-6 mr-2 text-current" />
          Running diff…
        </div>
      ) : result && (
        <>
          {/* Summary */}
          <div className="flex items-center gap-4 mb-4 text-sm">
            {result.totalAdded > 0 && (
              <span className="text-green-400 font-medium">+{result.totalAdded} added</span>
            )}
            {result.totalRemoved > 0 && (
              <span className="text-red-400 font-medium">−{result.totalRemoved} removed</span>
            )}
            {result.totalModified > 0 && (
              <span className="text-amber-400 font-medium">~{result.totalModified} modified</span>
            )}
            {totalChanges === 0 && (
              <span className="text-gray-500">No differences</span>
            )}
          </div>

          {result.truncated && (
            <div className="mb-4 p-3 bg-amber-900/30 border border-amber-700/50 rounded-lg text-sm text-amber-300">
              Showing the first 500 of {totalChanges.toLocaleString()} changes. The file tree below is partial — navigate into directories to explore what's visible.
            </div>
          )}

          {totalChanges === 0 ? (
            <EmptyState title="No differences" description="These two snapshots are identical." />
          ) : (
            <>
              {/* Breadcrumb */}
              <div className="flex items-center gap-1 mb-4 text-sm text-gray-400">
                <button
                  onClick={() => setCurrentPath("")}
                  className={currentPath === "" ? "text-gray-300" : "hover:text-gray-200 transition-colors"}
                >
                  root
                </button>
                {breadcrumbParts.map((part, i) => {
                  const path = "/" + breadcrumbParts.slice(0, i + 1).join("/");
                  const isCurrent = i === breadcrumbParts.length - 1;
                  return (
                    <span key={path} className="contents">
                      <span className="text-gray-700">/</span>
                      {isCurrent ? (
                        <span className="text-gray-300">{part}</span>
                      ) : (
                        <button
                          onClick={() => setCurrentPath(path)}
                          className="hover:text-gray-200 transition-colors"
                        >
                          {part}
                        </button>
                      )}
                    </span>
                  );
                })}
              </div>

              {children.length === 0 ? (
                <EmptyState title="No entries" description="No changes visible at this path level." />
              ) : (
                <div className="rounded-xl border border-gray-800 overflow-hidden">
                  <table className="w-full text-sm">
                    <thead>
                      <tr className="bg-gray-900 border-b border-gray-800 text-left">
                        <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Name</th>
                        <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-28">Change</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-gray-800">
                      {children.map((node) => {
                        const style = CHANGE_STYLES[node.change];
                        return (
                          <tr
                            key={node.fullPath}
                            className="hover:bg-gray-900/50 transition-colors"
                            onContextMenu={(e) => { e.preventDefault(); setContextMenu({ x: e.clientX, y: e.clientY, node }); }}
                          >
                            <td className="px-4 py-2.5">
                              <div className="flex items-center gap-2">
                                {node.isDir ? <DirIcon /> : <FileIcon />}
                                {node.isDir ? (
                                  <button
                                    onClick={() => setCurrentPath(node.fullPath)}
                                    className="text-gray-200 hover:text-gray-50 transition-colors text-left"
                                  >
                                    {node.name}
                                  </button>
                                ) : (
                                  <span className="text-gray-300">{node.name}</span>
                                )}
                              </div>
                            </td>
                            <td className="px-4 py-2.5">
                              <span className={`text-xs font-mono ${style.color}`}>
                                {style.icon} {style.label}
                              </span>
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}
            </>
          )}
        </>
      )}
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Restore…",
              onClick: () => openRestoreModal(contextMenu.node),
            },
          ]}
        />
      )}

      <Modal
        title="Restore"
        open={restoreTarget !== null}
        onClose={() => { if (!restoring) setRestoreTarget(null); }}
      >
        <p className="text-sm text-gray-300 mb-1">
          Restore <span className="font-mono text-blue-400 text-xs break-all">{restoreTarget?.path}</span> to a target directory.
        </p>
        <p className="text-xs text-gray-500 mb-4">
          Only files that conflict with the restored content will be overwritten; other files in the target are left untouched.
        </p>
        {restoreError && (
          <div className="mb-3 p-2 bg-red-900/30 border border-red-700 rounded text-sm text-red-300">{restoreError}</div>
        )}
        <div className="flex gap-2 mb-3">
          <div className="flex-1">
            <Input
              placeholder="Select a target directory…"
              value={targetDir}
              onChange={(e) => setTargetDir(e.target.value)}
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
          Restore {restoreTarget?.isDir ? "folder" : "file"} only (skip original path structure)
        </label>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setRestoreTarget(null)} disabled={restoring}>Cancel</Button>
          <Button loading={restoring} onClick={handleRestore} disabled={!targetDir}>Restore</Button>
        </div>
      </Modal>
    </div>
  );
}
