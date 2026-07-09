import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { cancelCopy, cancelRestore, checkRepo, clearSnapshotIndex, copySnapshot, deleteSnapshot, getRemoteAutoRefresh, getRestorePath, getSnapshotStats, getSnapshotIndexStatus, indexSnapshot, listRepos, listSnapshots, refreshSnapshots, restoreSnapshot, tagSnapshot, unlockRepo } from "../lib/invoke";
import type { CheckResult, Repository, RestoreProgress, Snapshot, SnapshotStats } from "../lib/types";
import { isRemoteRepo } from "../lib/types";
import { formatBytes, formatDate } from "../lib/format";
import Button from "../components/Button";
import ContextMenu, { type ContextMenuItemDef } from "../components/ContextMenu";
import Modal from "../components/Modal";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";
import Spinner from "../components/Spinner";

const PAGE_SIZE = 10;

export default function SnapshotsPage() {
  const navigate = useNavigate();
  const { repoId } = useParams<{ repoId: string }>();
  const [repo, setRepo] = useState<Repository | null>(null);
  const [allRepos, setAllRepos] = useState<Repository[]>([]);
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
  const [page, setPage] = useState(0);
  const [unlockConfirm, setUnlockConfirm] = useState(false);
  const [unlocking, setUnlocking] = useState(false);
  const [checking, setChecking] = useState(false);
  const [checkResult, setCheckResult] = useState<CheckResult | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<Snapshot | null>(null);
  const [restoreDir, setRestoreDir] = useState("");
  const [defaultRestoreDir, setDefaultRestoreDir] = useState("");
  const [restoring, setRestoring] = useState(false);
  const [restoreDone, setRestoreDone] = useState(false);
  const [restoreCancelled, setRestoreCancelled] = useState(false);
  const [restoreProgress, setRestoreProgress] = useState<RestoreProgress | null>(null);
  const restoreUnlistenRef = useRef<(() => void) | null>(null);
  const [copyTarget, setCopyTarget] = useState<Snapshot | null>(null);
  const [copyDestRepoId, setCopyDestRepoId] = useState("");
  const [copying, setCopying] = useState(false);
  const [copyDone, setCopyDone] = useState(false);
  const [copyCancelled, setCopyCancelled] = useState(false);
  const [contextMenu, setContextMenu] = useState<{ snap: Snapshot; x: number; y: number } | null>(null);
  const [statsTarget, setStatsTarget] = useState<Snapshot | null>(null);
  const [snapshotStats, setSnapshotStats] = useState<SnapshotStats | null>(null);
  const [statsLoading, setStatsLoading] = useState(false);
  const [statsError, setStatsError] = useState("");
  const [compareSource, setCompareSource] = useState<Snapshot | null>(null);
  const [compareTargetId, setCompareTargetId] = useState("");
  const [remoteAutoRefresh, setRemoteAutoRefresh] = useState(false);
  const [settingsReady, setSettingsReady] = useState(false);
  const [indexStatus, setIndexStatus] = useState<Record<string, string>>({});
  const [indexingTarget, setIndexingTarget] = useState<Snapshot | null>(null);
  const [indexingDone, setIndexingDone] = useState(false);
  const [indexingSuccess, setIndexingSuccess] = useState(true);

  // Multi-select state
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // Multi-delete state
  const [multiDeleteOpen, setMultiDeleteOpen] = useState(false);
  const [multiDeletePrune, setMultiDeletePrune] = useState(true);
  const [multiDeleting, setMultiDeleting] = useState(false);
  const [multiDeleteProgress, setMultiDeleteProgress] = useState({ current: 0, total: 0 });
  const [multiDeleteDone, setMultiDeleteDone] = useState(false);
  const [multiDeleteError, setMultiDeleteError] = useState("");

  // Multi-copy state
  const [multiCopyOpen, setMultiCopyOpen] = useState(false);
  const [multiCopyDestRepoId, setMultiCopyDestRepoId] = useState("");
  const [multiCopying, setMultiCopying] = useState(false);
  const [multiCopyProgress, setMultiCopyProgress] = useState({ current: 0, total: 0 });
  const [multiCopyDone, setMultiCopyDone] = useState(false);
  const [multiCopyCancelled, setMultiCopyCancelled] = useState(false);
  const [multiCopyError, setMultiCopyError] = useState("");
  const multiCopyCancelRef = useRef(false);

  const selectAllCheckboxRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    getRestorePath().then(setDefaultRestoreDir).catch(() => {});
    getRemoteAutoRefresh()
      .then(setRemoteAutoRefresh)
      .catch(() => {})
      .finally(() => setSettingsReady(true));
  }, []);

  useEffect(() => {
    if (!repoId) return;
    getSnapshotIndexStatus(repoId).then(setIndexStatus).catch(() => {});
  }, [repoId]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<{ snapshotId: string; repoId: string; success: boolean }>("index:done", (e) => {
      if (e.payload.repoId !== repoId) return;
      const { snapshotId, success } = e.payload;
      setIndexStatus((prev) => ({
        ...prev,
        [snapshotId]: success ? "complete" : "pending",
      }));
      setIndexingTarget((prev) => {
        if (prev?.id === snapshotId) {
          setIndexingDone(true);
          setIndexingSuccess(success);
        }
        return prev;
      });
    }).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [repoId]);

  useEffect(() => {
    if (!repoId) return;
    setSnapshots([]);
    setLoading(true);
    setSelectMode(false);
    setSelectedIds(new Set());
    listRepos().then((repos) => {
      setAllRepos(repos);
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

  // Cache paint only — see the settingsReady-gated effect below for the (single)
  // background refresh, which needs the resolved remoteAutoRefresh setting to decide
  // whether to run. Splitting these avoids refreshing twice on mount (once with the
  // default remoteAutoRefresh=false, once after the real setting loads).
  const load = useCallback(async () => {
    if (!repoId || !repo) return;
    setLoading(true);
    try {
      const cached = await listSnapshots(repoId);
      setSnapshots(cached.reverse());
    } finally {
      setLoading(false);
    }
  }, [repoId, repo]);

  useEffect(() => {
    load();
  }, [load]);

  // Fires exactly once per repo visit, after settingsReady flips true with the final
  // remoteAutoRefresh value already resolved.
  useEffect(() => {
    if (!repoId || !repo || !settingsReady) return;
    if (isRemoteRepo(repo.path) && !remoteAutoRefresh) return;
    setRefreshing(true);
    refreshSnapshots(repoId)
      .then((data) => setSnapshots(data.reverse()))
      .catch(() => {})
      .finally(() => setRefreshing(false));
  }, [repoId, repo, settingsReady, remoteAutoRefresh]);

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

  const handleCopy = async () => {
    if (!repoId || !copyTarget || !copyDestRepoId) return;
    setCopying(true);
    setCopyCancelled(false);
    try {
      await copySnapshot(repoId, copyDestRepoId, copyTarget.id);
      setCopyDone(true);
    } catch (err: any) {
      if (String(err).includes("cancelled")) {
        setCopyCancelled(true);
      } else {
        setError(String(err));
        setCopyTarget(null);
      }
    } finally {
      setCopying(false);
    }
  };

  const handlePickRestoreDir = async () => {
    const dir = await openDialog({ directory: true, multiple: false });
    if (typeof dir === "string") setRestoreDir(dir);
  };

  const handleRestore = async () => {
    if (!repoId || !restoreTarget || !restoreDir) return;
    setRestoring(true);
    setRestoreDone(false);
    setRestoreCancelled(false);
    setRestoreProgress(null);
    const unlisten = await listen<RestoreProgress>("restore:progress", (e) => {
      setRestoreProgress(e.payload);
    });
    restoreUnlistenRef.current = unlisten;
    try {
      await restoreSnapshot(repoId, restoreTarget.id, restoreDir);
      setRestoreDone(true);
    } catch (err: any) {
      if (String(err).includes("cancelled")) {
        setRestoreCancelled(true);
      } else {
        setError(String(err));
        setRestoreTarget(null);
      }
    } finally {
      unlisten();
      restoreUnlistenRef.current = null;
      setRestoring(false);
      setRestoreProgress(null);
    }
  };

  const handleMultiDelete = async () => {
    if (!repoId) return;
    const ids = Array.from(selectedIds);
    setMultiDeleting(true);
    setMultiDeleteError("");
    setMultiDeleteProgress({ current: 0, total: ids.length });
    for (let i = 0; i < ids.length; i++) {
      setMultiDeleteProgress({ current: i, total: ids.length });
      try {
        await deleteSnapshot(repoId, ids[i], multiDeletePrune);
      } catch (err: any) {
        setMultiDeleteError(String(err));
        setMultiDeleting(false);
        return;
      }
    }
    setMultiDeleteProgress({ current: ids.length, total: ids.length });
    setMultiDeleting(false);
    setMultiDeleteDone(true);
    setSelectedIds(new Set());
    await refresh();
  };

  const handleMultiCopy = async () => {
    if (!repoId || !multiCopyDestRepoId) return;
    const ids = Array.from(selectedIds);
    multiCopyCancelRef.current = false;
    setMultiCopying(true);
    setMultiCopyCancelled(false);
    setMultiCopyError("");
    setMultiCopyProgress({ current: 0, total: ids.length });
    for (let i = 0; i < ids.length; i++) {
      setMultiCopyProgress({ current: i, total: ids.length });
      if (multiCopyCancelRef.current) {
        setMultiCopyCancelled(true);
        setMultiCopying(false);
        return;
      }
      try {
        await copySnapshot(repoId, multiCopyDestRepoId, ids[i]);
      } catch (err: any) {
        if (String(err).includes("cancelled")) {
          setMultiCopyCancelled(true);
        } else {
          setMultiCopyError(String(err));
        }
        setMultiCopying(false);
        return;
      }
    }
    setMultiCopyProgress({ current: ids.length, total: ids.length });
    setMultiCopying(false);
    setMultiCopyDone(true);
    setSelectedIds(new Set());
  };

  const handleMultiCopyCancel = async () => {
    multiCopyCancelRef.current = true;
    await cancelCopy();
  };

  const exitSelectMode = () => {
    setSelectMode(false);
    setSelectedIds(new Set());
  };

  const filtered = useMemo(() => {
    if (!filter) return snapshots;
    const f = filter.toLowerCase();
    return snapshots.filter(
      (s) =>
        s.short_id.includes(filter) ||
        s.hostname.toLowerCase().includes(f) ||
        (s.tags ?? []).some((t) => t.toLowerCase().includes(f)) ||
        s.paths.some((p) => p.toLowerCase().includes(f))
    );
  }, [snapshots, filter]);

  const otherRepos = useMemo(
    () => allRepos.filter((r) => r.id !== repoId),
    [allRepos, repoId]
  );

  useEffect(() => {
    setPage(0);
  }, [filter, repoId]);

  useEffect(() => {
    if (filtered.length === 0) return;
    const lastPage = Math.ceil(filtered.length / PAGE_SIZE) - 1;
    if (page > lastPage) setPage(lastPage);
  }, [filtered.length, page]);

  const totalPages = Math.ceil(filtered.length / PAGE_SIZE);
  const pageEntries = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
  const allPageSelected = pageEntries.length > 0 && pageEntries.every((s) => selectedIds.has(s.id));
  const somePageSelected = pageEntries.some((s) => selectedIds.has(s.id));

  useEffect(() => {
    if (selectAllCheckboxRef.current) {
      selectAllCheckboxRef.current.indeterminate = somePageSelected && !allPageSelected;
    }
  }, [somePageSelected, allPageSelected]);

  const toggleSelectAll = () => {
    if (allPageSelected) {
      setSelectedIds((prev) => {
        const next = new Set(prev);
        pageEntries.forEach((s) => next.delete(s.id));
        return next;
      });
    } else {
      setSelectedIds((prev) => {
        const next = new Set(prev);
        pageEntries.forEach((s) => next.add(s.id));
        return next;
      });
    }
  };

  const toggleSelectOne = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

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
          <h1 className="text-xl font-semibold text-gray-100">Snapshots</h1>
          {repo && <p className="text-sm text-gray-500 mt-0.5">{repo.name}</p>}
        </div>
        <div className="flex items-center gap-3">
          <Input
            placeholder="Filter snapshots…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onClear={() => setFilter("")}
            className="w-56"
          />
          {refreshing && <span className="text-xs text-gray-500">Updating…</span>}
          {selectMode ? (
            <Button variant="secondary" onClick={exitSelectMode}>
              Cancel select
            </Button>
          ) : (
            <>
              <Button variant="secondary" onClick={() => setSelectMode(true)}>
                Select Multiple
              </Button>
              <Button variant="secondary" onClick={refresh} loading={refreshing}>
                Refresh
              </Button>
              <Button variant="secondary" onClick={handleCheck} loading={checking}>
                Check
              </Button>
              <Button variant="secondary" onClick={() => setUnlockConfirm(true)}>
                Unlock
              </Button>
            </>
          )}
        </div>
      </div>

      {repo && isRemoteRepo(repo.path) && !remoteAutoRefresh && (
        <div className="mb-4 p-3 bg-amber-900/20 border border-amber-700/50 rounded-lg text-sm text-amber-300 flex items-start gap-2">
          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 shrink-0 mt-0.5">
            <path fillRule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
          </svg>
          <span>
            Remote repositories don't auto-refresh — click <strong className="font-semibold">Refresh</strong> to load the latest snapshots, or enable auto-refresh in Settings.
          </span>
        </div>
      )}

      {selectMode && selectedIds.size > 0 && (
        <div className="mb-4 p-3 bg-amber-900/20 border border-amber-700/50 rounded-lg text-sm text-amber-300 flex items-center justify-between gap-2">
          <span>{selectedIds.size} snapshot{selectedIds.size !== 1 ? "s" : ""} selected</span>
          <div className="flex items-center gap-2">
            {otherRepos.length > 0 && (
              <Button
                variant="secondary"
                onClick={() => { setMultiCopyOpen(true); setMultiCopyDestRepoId(""); setMultiCopyDone(false); setMultiCopyCancelled(false); setMultiCopyError(""); }}
              >
                Copy selected
              </Button>
            )}
            <Button
              variant="danger"
              onClick={() => { setMultiDeleteOpen(true); setMultiDeleteDone(false); setMultiDeleteError(""); }}
            >
              Delete selected
            </Button>
          </div>
        </div>
      )}

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
        <>
          <div className="rounded-xl border border-gray-800 overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-gray-900 border-b border-gray-800 text-left">
                  {selectMode && (
                    <th className="px-4 py-3 w-10">
                      <input
                        ref={selectAllCheckboxRef}
                        type="checkbox"
                        checked={allPageSelected}
                        onChange={toggleSelectAll}
                        className="rounded bg-gray-700 border-gray-600"
                      />
                    </th>
                  )}
                  <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">ID</th>
                  <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Date</th>
                  <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Host</th>
                  <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Paths</th>
                  <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Tags</th>
                  {!selectMode && (
                    <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-20">Actions</th>
                  )}
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-800">
                {pageEntries.map((snap) => (
                  <tr
                    key={snap.id}
                    className={`hover:bg-gray-900/50 transition-colors ${selectMode ? "cursor-pointer" : ""} ${selectedIds.has(snap.id) ? "bg-gray-900/40" : ""}`}
                    onClick={selectMode ? () => toggleSelectOne(snap.id) : undefined}
                    onContextMenu={(e) => {
                      if (selectMode) return;
                      e.preventDefault();
                      e.stopPropagation();
                      setContextMenu({ snap, x: e.clientX, y: e.clientY });
                    }}
                  >
                    {selectMode && (
                      <td className="px-4 py-3" onClick={(e) => e.stopPropagation()}>
                        <input
                          type="checkbox"
                          checked={selectedIds.has(snap.id)}
                          onChange={() => toggleSelectOne(snap.id)}
                          className="rounded bg-gray-700 border-gray-600"
                        />
                      </td>
                    )}
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
                            {!selectMode && (
                              <button
                                onClick={(e) => { e.stopPropagation(); handleRemoveTag(snap, tag); }}
                                className="text-gray-500 hover:text-red-300 transition-colors"
                              >
                                ×
                              </button>
                            )}
                          </span>
                        ))}
                        {!selectMode && (
                          <button
                            onClick={() => { setTagTarget(snap); setNewTag(""); }}
                            className="text-xs text-gray-600 hover:text-blue-400 transition-colors px-1"
                          >
                            + tag
                          </button>
                        )}
                      </div>
                    </td>
                    {!selectMode && (
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
                            title="Search files"
                            onClick={() => navigate(`/snapshots/${repoId}/${snap.id}/search`, { state: { snapshot: snap } })}
                            className="p-1.5 rounded text-gray-400 hover:text-blue-400 hover:bg-gray-800 transition-colors"
                          >
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                              <path fillRule="evenodd" d="M9 3.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11ZM2 9a7 7 0 1 1 12.452 4.391l3.328 3.329a.75.75 0 1 1-1.06 1.06l-3.329-3.328A7 7 0 0 1 2 9Z" clipRule="evenodd" />
                            </svg>
                          </button>
                          <button
                            title="Restore snapshot"
                            onClick={() => { setRestoreTarget(snap); setRestoreDir(defaultRestoreDir); setRestoreDone(false); }}
                            className="p-1.5 rounded text-gray-400 hover:text-green-400 hover:bg-gray-800 transition-colors"
                          >
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                              <path d="M10.75 2.75a.75.75 0 00-1.5 0v8.614L6.295 8.235a.75.75 0 10-1.09 1.03l4.25 4.5a.75.75 0 001.09 0l4.25-4.5a.75.75 0 00-1.09-1.03l-2.955 3.129V2.75z" />
                              <path d="M3.5 12.75a.75.75 0 00-1.5 0v2.5A2.75 2.75 0 004.75 18h10.5A2.75 2.75 0 0018 15.25v-2.5a.75.75 0 00-1.5 0v2.5c0 .69-.56 1.25-1.25 1.25H4.75c-.69 0-1.25-.56-1.25-1.25v-2.5z" />
                            </svg>
                          </button>
                          <button
                            title="Copy to repository"
                            onClick={() => { setCopyTarget(snap); setCopyDestRepoId(""); setCopyDone(false); }}
                            className="p-1.5 rounded text-gray-400 hover:text-purple-400 hover:bg-gray-800 transition-colors"
                            disabled={otherRepos.length === 0}
                          >
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                              <path d="M7 3.5A1.5 1.5 0 018.5 2h3.879a1.5 1.5 0 011.06.44l3.122 3.12A1.5 1.5 0 0117 6.622V12.5a1.5 1.5 0 01-1.5 1.5h-1v-3.379a3 3 0 00-.879-2.121L10.5 5.379A3 3 0 008.379 4.5H7v-1z" />
                              <path d="M4.5 6A1.5 1.5 0 003 7.5v9A1.5 1.5 0 004.5 18h7a1.5 1.5 0 001.5-1.5v-5.879a1.5 1.5 0 00-.44-1.06L9.44 6.439A1.5 1.5 0 008.378 6H4.5z" />
                            </svg>
                          </button>
                          <button
                            title="Delete snapshot"
                            onClick={() => setDeleteTarget(snap)}
                            className="p-1.5 rounded text-gray-600 hover:text-red-300 hover:bg-gray-800 transition-colors"
                          >
                            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                              <path fillRule="evenodd" d="M8.75 1A2.75 2.75 0 006 3.75v.443c-.795.077-1.584.176-2.365.298a.75.75 0 10.23 1.482l.149-.022.841 10.518A2.75 2.75 0 007.596 19h4.807a2.75 2.75 0 002.742-2.53l.841-10.52.149.023a.75.75 0 00.23-1.482A41.03 41.03 0 0014 4.193v-.443A2.75 2.75 0 0011.25 1h-2.5zM10 4c.84 0 1.673.025 2.5.075V3.75c0-.69-.56-1.25-1.25-1.25h-2.5c-.69 0-1.25.56-1.25 1.25v.325C8.327 4.025 9.16 4 10 4zM8.58 7.72a.75.75 0 00-1.5.06l.3 7.5a.75.75 0 101.5-.06l-.3-7.5zm4.34.06a.75.75 0 10-1.5-.06l-.3 7.5a.75.75 0 101.5.06l.3-7.5z" clipRule="evenodd" />
                            </svg>
                          </button>
                        </div>
                      </td>
                    )}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {totalPages > 1 && (
            <div className="flex items-center justify-between mt-4">
              <p className="text-sm text-gray-500">
                Page {page + 1} of {totalPages} · {filtered.length} snapshot{filtered.length !== 1 ? "s" : ""}
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
      )}

      {/* Single-snapshot delete modal */}
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

      {/* Multi-delete modal */}
      <Modal
        title="Delete Snapshots"
        open={multiDeleteOpen}
        onClose={() => { if (!multiDeleting) { setMultiDeleteOpen(false); setMultiDeleteDone(false); setMultiDeleteError(""); } }}
      >
        {multiDeleteDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              All snapshots deleted
            </div>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiDeleteOpen(false); setMultiDeleteDone(false); exitSelectMode(); }}>Close</Button>
            </div>
          </>
        ) : multiDeleteError ? (
          <>
            <p className="text-sm text-red-300 mb-4">{multiDeleteError}</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiDeleteOpen(false); setMultiDeleteError(""); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Delete {selectedIds.size} snapshot{selectedIds.size !== 1 ? "s" : ""}? This cannot be undone.
            </p>
            <label className="flex items-center gap-2 text-sm text-gray-300 mb-4 cursor-pointer">
              <input
                type="checkbox"
                checked={multiDeletePrune}
                onChange={(e) => setMultiDeletePrune(e.target.checked)}
                className="rounded bg-gray-700 border-gray-600"
                disabled={multiDeleting}
              />
              Also run <span className="font-mono text-xs">restic prune</span> after each forget
            </label>
            {multiDeleting && (
              <div className="mb-4">
                <div className="flex justify-between text-xs text-gray-400 mb-1">
                  <span>Deleting snapshot {multiDeleteProgress.current + 1} of {multiDeleteProgress.total}…</span>
                </div>
                <div className="w-full bg-gray-800 rounded-full h-2">
                  <div
                    className="bg-red-500 h-2 rounded-full transition-all duration-300"
                    style={{ width: `${Math.round((multiDeleteProgress.current / multiDeleteProgress.total) * 100)}%` }}
                  />
                </div>
              </div>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setMultiDeleteOpen(false)} disabled={multiDeleting}>Cancel</Button>
              <Button variant="danger" loading={multiDeleting} onClick={handleMultiDelete}>
                Delete {selectedIds.size} snapshot{selectedIds.size !== 1 ? "s" : ""}
              </Button>
            </div>
          </>
        )}
      </Modal>

      {/* Multi-copy modal */}
      <Modal
        title="Copy Snapshots"
        open={multiCopyOpen}
        onClose={() => { if (!multiCopying) { setMultiCopyOpen(false); setMultiCopyDone(false); setMultiCopyCancelled(false); setMultiCopyError(""); } }}
      >
        {multiCopyDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              All snapshots copied
            </div>
            <p className="text-sm text-gray-400 mb-4">
              {multiCopyProgress.total} snapshot{multiCopyProgress.total !== 1 ? "s were" : " was"} copied to{" "}
              <span className="text-gray-300">{otherRepos.find((r) => r.id === multiCopyDestRepoId)?.name ?? multiCopyDestRepoId}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiCopyOpen(false); setMultiCopyDone(false); exitSelectMode(); }}>Close</Button>
            </div>
          </>
        ) : multiCopyCancelled ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-amber-300">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
              </svg>
              Copy cancelled
            </div>
            <p className="text-sm text-gray-400 mb-3">
              Stopped after {multiCopyProgress.current} of {multiCopyProgress.total} snapshot{multiCopyProgress.total !== 1 ? "s" : ""}. No further snapshots were copied.
            </p>
            <p className="text-xs text-gray-500 mb-4">
              Any partially transferred data will remain as unreferenced blobs until you run{" "}
              <span className="font-mono text-gray-400">restic prune</span> on{" "}
              <span className="text-gray-300">{otherRepos.find((r) => r.id === multiCopyDestRepoId)?.name ?? "the destination"}</span>.
              You may also need to unlock that repository.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiCopyOpen(false); setMultiCopyCancelled(false); }}>Close</Button>
            </div>
          </>
        ) : multiCopyError ? (
          <>
            <p className="text-sm text-red-300 mb-4">{multiCopyError}</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiCopyOpen(false); setMultiCopyError(""); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Copy {selectedIds.size} snapshot{selectedIds.size !== 1 ? "s" : ""} to another repository.
              Only data not already present in the destination will be transferred.
            </p>
            <div className="mb-4">
              <label className="block text-xs text-gray-500 mb-1.5 uppercase tracking-wider font-medium">Destination repository</label>
              <div className="relative">
                <select
                  value={multiCopyDestRepoId}
                  onChange={(e) => setMultiCopyDestRepoId(e.target.value)}
                  className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-200 focus:outline-none focus:ring-2 focus:ring-blue-500"
                  disabled={multiCopying}
                >
                  <option value="">Select a repository…</option>
                  {otherRepos.map((r) => (
                    <option key={r.id} value={r.id}>{r.name} — {r.path}</option>
                  ))}
                </select>
                <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">▾</div>
              </div>
            </div>
            {multiCopying && (
              <div className="mb-4">
                <div className="flex justify-between text-xs text-gray-400 mb-1">
                  <span>Copying snapshot {multiCopyProgress.current + 1} of {multiCopyProgress.total}…</span>
                </div>
                <div className="w-full bg-gray-800 rounded-full h-2">
                  <div
                    className="bg-purple-500 h-2 rounded-full transition-all duration-300"
                    style={{ width: `${Math.round((multiCopyProgress.current / multiCopyProgress.total) * 100)}%` }}
                  />
                </div>
              </div>
            )}
            <div className="flex justify-end gap-2">
              {multiCopying ? (
                <Button variant="danger" onClick={handleMultiCopyCancel}>Stop</Button>
              ) : (
                <Button variant="secondary" onClick={() => setMultiCopyOpen(false)}>Cancel</Button>
              )}
              <Button onClick={handleMultiCopy} loading={multiCopying} disabled={!multiCopyDestRepoId}>
                Copy {selectedIds.size} snapshot{selectedIds.size !== 1 ? "s" : ""}
              </Button>
            </div>
          </>
        )}
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
              <div className={`flex flex-col items-center justify-center py-8 gap-2 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-300"}`}>
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
                <div className={`flex items-center gap-2 mb-4 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-300"}`}>
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                    <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                  </svg>
                  Errors found
                </div>
                <div className="mb-4 space-y-2">
                  {checkResult.errors.map((err, i) => (
                    <div key={i} className="text-xs font-mono bg-red-900/30 border border-red-700 rounded p-2 text-red-300 whitespace-pre-wrap break-all">
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
        onClose={() => { if (!restoring) { setRestoreTarget(null); setRestoreDone(false); setRestoreCancelled(false); } }}
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
        ) : restoreCancelled ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-amber-300">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
              </svg>
              Restore cancelled
            </div>
            <p className="text-sm text-gray-400 mb-4">
              The restore was stopped before completing. Files already written to{" "}
              <span className="font-mono text-gray-300 break-all">{restoreDir}</span> were left in place; the rest were not restored.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setRestoreTarget(null); setRestoreCancelled(false); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Restore all files from snapshot{" "}
              <span className="font-mono text-blue-400">{restoreTarget?.short_id}</span> to a target directory.
              Only files that conflict with the restored content will be overwritten; other files in the target are left untouched.
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
              {restoring ? (
                <Button variant="danger" onClick={() => cancelRestore()}>Stop</Button>
              ) : (
                <Button variant="secondary" onClick={() => setRestoreTarget(null)}>Cancel</Button>
              )}
              <Button onClick={handleRestore} loading={restoring} disabled={!restoreDir || restoring}>
                Restore
              </Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Copy Snapshot"
        open={copyTarget !== null}
        onClose={() => { if (!copying) { setCopyTarget(null); setCopyDone(false); setCopyCancelled(false); } }}
      >
        {copyDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Copy complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Snapshot <span className="font-mono text-blue-400">{copyTarget?.short_id}</span> was copied to{" "}
              <span className="text-gray-300">{allRepos.find((r) => r.id === copyDestRepoId)?.name ?? copyDestRepoId}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setCopyTarget(null); setCopyDone(false); }}>Close</Button>
            </div>
          </>
        ) : copyCancelled ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-amber-300">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M8.485 2.495c.673-1.167 2.357-1.167 3.03 0l6.28 10.875c.673 1.167-.17 2.625-1.516 2.625H3.72c-1.347 0-2.189-1.458-1.515-2.625L8.485 2.495zM10 5a.75.75 0 01.75.75v3.5a.75.75 0 01-1.5 0v-3.5A.75.75 0 0110 5zm0 9a1 1 0 100-2 1 1 0 000 2z" clipRule="evenodd" />
              </svg>
              Copy cancelled
            </div>
            <p className="text-sm text-gray-400 mb-3">
              The copy was stopped before completing. No snapshot was written to the destination.
            </p>
            <p className="text-xs text-gray-500 mb-4">
              Any partially transferred data will remain as unreferenced blobs until you run{" "}
              <span className="font-mono text-gray-400">restic prune</span> on{" "}
              <span className="text-gray-300">{allRepos.find((r) => r.id === copyDestRepoId)?.name ?? "the destination"}</span>.
              You may also need to unlock that repository.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setCopyTarget(null); setCopyCancelled(false); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Copy snapshot <span className="font-mono text-blue-400">{copyTarget?.short_id}</span> to another repository.
              Only data not already present in the destination will be transferred.
            </p>
            <div className="mb-4">
              <label className="block text-xs text-gray-500 mb-1.5 uppercase tracking-wider font-medium">Destination repository</label>
              <div className="relative">
                <select
                  value={copyDestRepoId}
                  onChange={(e) => setCopyDestRepoId(e.target.value)}
                  className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-200 focus:outline-none focus:ring-2 focus:ring-blue-500"
                  disabled={copying}
                >
                  <option value="">Select a repository…</option>
                  {allRepos
                    .filter((r) => r.id !== repoId)
                    .map((r) => (
                      <option key={r.id} value={r.id}>{r.name} — {r.path}</option>
                    ))}
                </select>
                <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">▾</div>
              </div>
            </div>
            {copying && (
              <div className="mb-4">
                <div className="text-xs text-gray-400 mb-1">Copying — this may take a while…</div>
                <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                  <div className="h-2 w-1/3 rounded-full bg-purple-500 animate-[slide_1.4s_ease-in-out_infinite]" />
                </div>
              </div>
            )}
            <div className="flex justify-end gap-2">
              {copying ? (
                <Button variant="danger" onClick={() => cancelCopy()}>Stop</Button>
              ) : (
                <Button variant="secondary" onClick={() => setCopyTarget(null)}>Cancel</Button>
              )}
              <Button onClick={handleCopy} loading={copying} disabled={!copyDestRepoId}>
                Copy
              </Button>
            </div>
          </>
        )}
      </Modal>

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Browse Files",
              onClick: () => navigate(`/snapshots/${repoId}/${contextMenu.snap.id}/browse`, { state: { snapshot: contextMenu.snap } }),
            },
            {
              label: "Search Files",
              onClick: () => navigate(`/snapshots/${repoId}/${contextMenu.snap.id}/search`, { state: { snapshot: contextMenu.snap } }),
            },
            {
              label: "Restore…",
              onClick: () => { setRestoreTarget(contextMenu.snap); setRestoreDir(defaultRestoreDir); setRestoreDone(false); },
            },
            {
              label: "Copy to Repository…",
              disabled: allRepos.filter((r) => r.id !== repoId).length === 0,
              onClick: () => { setCopyTarget(contextMenu.snap); setCopyDestRepoId(""); setCopyDone(false); },
            },
            {
              label: "Add Tag…",
              onClick: () => { setTagTarget(contextMenu.snap); setNewTag(""); },
            },
            {
              label: "Compare with…",
              disabled: snapshots.length < 2,
              onClick: () => {
                const snap = contextMenu.snap;
                const idx = snapshots.findIndex((s) => s.id === snap.id);
                const adjacent = snapshots[idx + 1] ?? snapshots[idx - 1] ?? null;
                setCompareSource(snap);
                setCompareTargetId(adjacent?.id ?? "");
              },
            },
            { separator: true },
            ...(indexStatus[contextMenu.snap.id] === "complete"
              ? [{
                  label: "Remove Index",
                  onClick: async () => {
                    const snap = contextMenu.snap;
                    setContextMenu(null);
                    try {
                      await clearSnapshotIndex(repoId!, snap.id);
                      setIndexStatus((prev) => {
                        const next = { ...prev };
                        delete next[snap.id];
                        return next;
                      });
                    } catch {
                      // Reconcile true state on failure.
                      getSnapshotIndexStatus(repoId!).then(setIndexStatus).catch(() => {});
                    }
                  },
                }]
              : [{
                  label: "Index Snapshot",
                  disabled: indexStatus[contextMenu.snap.id] === "in_progress",
                  onClick: async () => {
                    const snap = contextMenu.snap;
                    setContextMenu(null);
                    let started = false;
                    try { started = await indexSnapshot(repoId!, snap.id); }
                    catch { started = false; }
                    if (!started) {
                      // Already complete or in progress (e.g. warmer mid-flight):
                      // reconcile the row's true state instead of opening a modal.
                      getSnapshotIndexStatus(repoId!).then(setIndexStatus).catch(() => {});
                      return;
                    }
                    setIndexingTarget(snap);
                    setIndexingDone(false);
                    setIndexingSuccess(true);
                    setIndexStatus((prev) => ({ ...prev, [snap.id]: "in_progress" }));
                  },
                }]),
            {
              label: "Snapshot Stats",
              onClick: () => {
                const snap = contextMenu.snap;
                setStatsTarget(snap);
                setSnapshotStats(null);
                setStatsError("");
                setStatsLoading(true);
                getSnapshotStats(repoId!, snap.id)
                  .then(setSnapshotStats)
                  .catch((err) => setStatsError(String(err)))
                  .finally(() => setStatsLoading(false));
              },
            },
            { separator: true },
            {
              label: "Delete",
              variant: "danger",
              onClick: () => setDeleteTarget(contextMenu.snap),
            },
          ] satisfies ContextMenuItemDef[]}
        />
      )}

      <Modal
        title={`Snapshot Stats: ${statsTarget?.short_id ?? ""}`}
        open={statsTarget !== null}
        onClose={() => { setStatsTarget(null); setSnapshotStats(null); setStatsError(""); }}
      >
        {statsLoading ? (
          <div className="flex flex-col items-center justify-center py-10 gap-3 text-sm text-gray-400">
            <Spinner className="w-6 h-6 text-blue-400" />
            Running restic stats…
          </div>
        ) : statsError ? (
          <>
            <p className="text-sm text-red-300 mb-4">{statsError}</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setStatsTarget(null); setStatsError(""); }}>Close</Button>
            </div>
          </>
        ) : snapshotStats && (
          <>
            <div className="space-y-3 mb-6">
              <div className="flex items-center justify-between py-2 border-b border-gray-800">
                <span className="text-sm text-gray-400">Total size</span>
                <span className="text-sm font-medium text-gray-100">{formatBytes(snapshotStats.totalSize)}</span>
              </div>
              <div className="flex items-center justify-between py-2 border-b border-gray-800">
                <span className="text-sm text-gray-400">File count</span>
                <span className="text-sm font-medium text-gray-100">{snapshotStats.totalFileCount.toLocaleString()}</span>
              </div>
            </div>
            <p className="text-xs text-gray-600 mb-4">Size reflects all data in this snapshot, including data shared with other snapshots.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setStatsTarget(null); setSnapshotStats(null); }}>Close</Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Index Snapshot"
        open={indexingTarget !== null}
        onClose={() => { setIndexingTarget(null); setIndexingDone(false); }}
      >
        {indexingDone ? (
          indexingSuccess ? (
            <>
              <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                  <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
                </svg>
                Indexing complete
              </div>
              <p className="text-sm text-gray-400 mb-4">
                Snapshot <span className="font-mono text-blue-400">{indexingTarget?.short_id}</span> has been indexed and is ready to browse.
              </p>
              <div className="flex justify-end">
                <Button variant="secondary" onClick={() => { setIndexingTarget(null); setIndexingDone(false); }}>Close</Button>
              </div>
            </>
          ) : (
            <>
              <p className="text-sm text-red-300 mb-4">Indexing failed for snapshot <span className="font-mono text-blue-400">{indexingTarget?.short_id}</span>.</p>
              <div className="flex justify-end">
                <Button variant="secondary" onClick={() => { setIndexingTarget(null); setIndexingDone(false); }}>Close</Button>
              </div>
            </>
          )
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Indexing snapshot <span className="font-mono text-blue-400">{indexingTarget?.short_id}</span>… This may take a moment depending on the number of files.
            </p>
            <div className="mb-4">
              <div className="text-xs text-gray-400 mb-1">Building file index…</div>
              <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                <div className="h-2 w-1/3 rounded-full bg-blue-500 animate-[slide_1.4s_ease-in-out_infinite]" />
              </div>
            </div>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexingTarget(null)}>Close</Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Compare Snapshots"
        open={compareSource !== null}
        onClose={() => setCompareSource(null)}
      >
        <p className="text-sm text-gray-300 mb-4">
          Compare <span className="font-mono text-blue-400">{compareSource?.short_id}</span> against:
        </p>
        <div className="relative mb-4">
          <select
            value={compareTargetId}
            onChange={(e) => setCompareTargetId(e.target.value)}
            className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-md px-3 py-2 text-sm text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500 pr-8"
          >
            <option value="" disabled>Select a snapshot…</option>
            {snapshots
              .filter((s) => s.id !== compareSource?.id)
              .map((s) => (
                <option key={s.id} value={s.id}>
                  {s.short_id} — {formatDate(s.time)} — {s.hostname}
                </option>
              ))}
          </select>
          <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">▾</div>
        </div>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setCompareSource(null)}>Cancel</Button>
          <Button
            disabled={!compareTargetId}
            onClick={() => {
              if (!repoId || !compareSource || !compareTargetId) return;
              const targetSnap = snapshots.find((s) => s.id === compareTargetId);
              if (!targetSnap) return;
              const sourceIsOlder = new Date(compareSource.time) <= new Date(targetSnap.time);
              const [idA, idB, snapA, snapB] = sourceIsOlder
                ? [compareSource.id, compareTargetId, compareSource, targetSnap]
                : [compareTargetId, compareSource.id, targetSnap, compareSource];
              navigate(
                `/snapshots/${repoId}/diff/${idA}/${idB}`,
                { state: { snapshotA: snapA, snapshotB: snapB } }
              );
              setCompareSource(null);
            }}
          >
            Compare
          </Button>
        </div>
      </Modal>
    </div>
  );
}
