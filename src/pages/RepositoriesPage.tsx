import { useEffect, useRef, useState, type MouseEvent, type FormEvent } from "react";
import ContextMenu, { type ContextMenuItemDef } from "../components/ContextMenu";
import { useNavigate, useSearchParams } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { useActivity } from "../lib/activity";
import {
  addRepo,
  cancelIndexBatch,
  cancelMirror,
  cancelPrune,
  checkRepo,
  getActiveIndexBatch,
  getRepoPassword,
  getRepoStats,
  getSnapshotIndexStatus,
  indexSnapshotsBatch,
  initRepo,
  listRepos,
  listSnapshots,
  mirrorRepo,
  pruneRepo,
  refreshRepoStats,
  refreshSnapshots,
  removeRepo,
  renameRepo,
  updateRepoPassword,
  updateRepoPath,
  testRepoConnection,
} from "../lib/invoke";
import type { ActiveIndexBatchStatus, CheckResult, Repository, ResticStats, TaskEvent } from "../lib/types";
import { INDEX_BATCH_ALREADY_ACTIVE_ERROR, isRemoteRepo } from "../lib/types";
import { formatBytes, formatRelative, formatTimestamp } from "../lib/format";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";
import Spinner from "../components/Spinner";
import ProgressBar from "../components/ProgressBar";

type ModalMode = "add" | "init" | null;

export default function RepositoriesPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [repos, setRepos] = useState<Repository[]>([]);
  const [statsMap, setStatsMap] = useState<Record<string, ResticStats | null>>({});
  // Per-row spinner (statsRefreshing) and failure marker (statsFailed) are both bus-driven
  // rather than local state, so they reflect a refresh that's still running/failed even if
  // the user navigated away and back. No error text is tracked — see activity.tsx's module
  // doc comment for why a plain boolean marker is the deliberate choice.
  // statsRefreshAllProgress/setStatsRefreshAllProgress live in ActivityProvider (not local
  // state) so the Activity panel can show this button's batch progress too, and so it survives
  // navigating away from this page mid-refresh — see activity.tsx's doc comment on that field
  // for why it's tracked separately from statsRefreshing (which is always 1 throughout this
  // operation, since it refreshes repos one at a time, not in parallel — see handleRefreshAll).
  const { statsRefreshing, statsFailed, statsRefreshAllProgress, setStatsRefreshAllProgress } = useActivity();
  // Deliberately no local `refreshingAll` state — an earlier version had one, gating the
  // buttons below via `disabled={refreshingAll}`. It reset to `false` on every remount
  // (plain useState), while `statsRefreshAllProgress` lives in ActivityProvider specifically
  // so the batch survives navigating away and back — so after a remount mid-refresh, the
  // button correctly showed live progress but was clickable again, letting a second
  // concurrent `handleRefreshAll` loop start. `statsRefreshAllProgress != null` is the same
  // "is a refresh-all running" fact, sourced from state that's actually accurate no matter
  // when this component (re)mounted.
  const refreshingAll = statsRefreshAllProgress != null;
  const [modalMode, setModalMode] = useState<ModalMode>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [form, setForm] = useState({ name: "", path: "", password: "" });
  const [noPassword, setNoPassword] = useState(false);
  const [pathMode, setPathMode] = useState<"local" | "remote">("local");
  const [editTarget, setEditTarget] = useState<Repository | null>(null);
  const [editName, setEditName] = useState("");
  const [editPath, setEditPath] = useState("");
  const [editPassword, setEditPassword] = useState("");
  const [editNoPassword, setEditNoPassword] = useState(false);
  const [editTestResult, setEditTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [editTesting, setEditTesting] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Repository | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [mirrorSource, setMirrorSource] = useState<Repository | null>(null);
  const [mirrorDestId, setMirrorDestId] = useState("");
  const [mirroring, setMirroring] = useState(false);
  const [mirrorDone, setMirrorDone] = useState(false);
  const [mirrorCancelled, setMirrorCancelled] = useState(false);
  const [mirrorError, setMirrorError] = useState("");
  const [mirrorElapsed, setMirrorElapsed] = useState(0);
  const mirrorStartRef = useRef<number>(0);
  const [contextMenu, setContextMenu] = useState<{ repo: Repository; x: number; y: number } | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkResult, setCheckResult] = useState<CheckResult | null>(null);
  const [checkRepoName, setCheckRepoName] = useState("");
  const [pruneTarget, setPruneTarget] = useState<Repository | null>(null);
  // Decoupled from pruneTarget so the modal can be hidden while a prune keeps running in the
  // background (see closePruneModal) — pruneTarget stays set so reopening shows the same
  // repo's live progress instead of a blank state. Mirrors SettingsPage's "Prune All" modal.
  const [pruneModalOpen, setPruneModalOpen] = useState(false);
  const [pruning, setPruning] = useState(false);
  const [pruneDone, setPruneDone] = useState(false);
  const [pruneCancelled, setPruneCancelled] = useState(false);
  const [pruneError, setPruneError] = useState("");
  const [pruneElapsed, setPruneElapsed] = useState(0);
  const pruneStartRef = useRef<number>(0);
  // "Index All Snapshots" modal — mirrors RepoSearchPage.tsx's own batch-tracking state
  // (deliberate duplication, see CLAUDE.md's "Known, deferred frontend duplication"), scoped to
  // whichever repo the context menu most recently targeted rather than a whole page, since this
  // page can trigger a batch for any repo. `indexAllRepo` persists independently of
  // `indexAllOpen` so the modal can be dismissed while its batch keeps running (progress is
  // still visible via ActivityPanel's activeIndexBatches in the meantime), and reopening the
  // context menu for the same repo resumes tracking instead of restarting.
  const [indexAllRepo, setIndexAllRepo] = useState<Repository | null>(null);
  const [indexAllOpen, setIndexAllOpen] = useState(false);
  const [indexAllError, setIndexAllError] = useState("");
  // True when handleIndexAll's fetch found nothing left to index (repo fully indexed, or has no
  // snapshots at all) — lets the modal explain that instead of silently closing right after it
  // opened, which otherwise reads as an unexplained flash.
  const [indexAllNothingToDo, setIndexAllNothingToDo] = useState(false);
  const [indexAllTargets, setIndexAllTargets] = useState<string[]>([]);
  const [indexAllCompleted, setIndexAllCompleted] = useState<Set<string>>(new Set());
  const [indexAllFailedSet, setIndexAllFailedSet] = useState<Set<string>>(new Set());
  const [indexAllStopped, setIndexAllStopped] = useState(false);
  const indexAllTargetsRef = useRef<string[]>([]);
  type IndexAllBatchState =
    | { kind: "idle" }
    | { kind: "starting" }
    | { kind: "queued"; operationId: string }
    | { kind: "running"; operationId: string };
  const [indexAllBatchState, setIndexAllBatchState] = useState<IndexAllBatchState>({ kind: "idle" });
  const indexAllBatchActive = indexAllBatchState.kind !== "idle";
  const indexAllQueued = indexAllBatchState.kind === "queued";
  const indexAllOperationId =
    indexAllBatchState.kind === "queued" || indexAllBatchState.kind === "running" ? indexAllBatchState.operationId : null;
  const indexAllTotal = indexAllTargets.length;
  const indexAllDoneCount = indexAllCompleted.size + indexAllFailedSet.size;
  const indexAllFailedCount = indexAllFailedSet.size;
  // Whether each repo has at least one snapshot that isn't fully indexed — drives the "Index All
  // Snapshots" context-menu item's disabled state. Missing key = not yet checked; treated as
  // enabled (fail open) rather than blocking the menu item until this loads, since clicking it
  // with nothing to do just shows the "already indexed" modal branch.
  const [repoNeedsIndexing, setRepoNeedsIndexing] = useState<Record<string, boolean>>({});

  const load = () =>
    listRepos()
      .then((r) => { setRepos(r); return r; })
      .catch(() => [] as Repository[]);

  const fetchStatsForLocal = (repoList: Repository[]) => {
    for (const repo of repoList) {
      getRepoStats(repo.id)
        .then((s) => setStatsMap((prev) => ({ ...prev, [repo.id]: s })))
        .catch(() => setStatsMap((prev) => ({ ...prev, [repo.id]: null })));
    }
  };

  useEffect(() => {
    load().then(fetchStatsForLocal);
  }, []);

  // Populates repoNeedsIndexing for every currently-known repo — cache-only reads (no restic
  // calls, see CLAUDE.md's Restic Integration section on list_snapshots/get_snapshot_index_status),
  // so it's cheap to recompute whenever the repo list changes (add/edit/delete), not just on mount.
  useEffect(() => {
    let cancelled = false;
    for (const repo of repos) {
      Promise.all([listSnapshots(repo.id), getSnapshotIndexStatus(repo.id)])
        .then(([snaps, statusMap]) => {
          if (cancelled) return;
          const needsIndexing = snaps.some((s) => statusMap[s.id] !== "complete");
          setRepoNeedsIndexing((prev) => ({ ...prev, [repo.id]: needsIndexing }));
        })
        .catch(() => {});
    }
    return () => { cancelled = true; };
  }, [repos]);

  // Keeps repoNeedsIndexing live once loaded: a per-snapshot index finishing (whether from this
  // page's own "Index All", RepoSearchPage's, or the background auto-indexer) can flip a repo
  // to fully indexed, and a fresh snapshots:refreshed (new backup picked up by the cache warmer)
  // can flip it back to needing indexing.
  useEffect(() => {
    let cancelled = false;
    let unlistenTask: (() => void) | undefined;
    let unlistenSnapshots: (() => void) | undefined;

    const refresh = (repoId: string) => {
      Promise.all([listSnapshots(repoId), getSnapshotIndexStatus(repoId)])
        .then(([snaps, statusMap]) => {
          if (cancelled) return;
          const needsIndexing = snaps.some((s) => statusMap[s.id] !== "complete");
          setRepoNeedsIndexing((prev) => ({ ...prev, [repoId]: needsIndexing }));
        })
        .catch(() => {});
    };

    listen<TaskEvent>("task", (e) => {
      const t = e.payload;
      if (t.kind === "index" && t.phase === "finished") refresh(t.repoId);
    }).then((u) => {
      if (cancelled) { u(); return; }
      unlistenTask = u;
    });

    listen<{ repoId: string }>("snapshots:refreshed", (e) => {
      refresh(e.payload.repoId);
    }).then((u) => {
      if (cancelled) { u(); return; }
      unlistenSnapshots = u;
    });

    return () => { cancelled = true; unlistenTask?.(); unlistenSnapshots?.(); };
  }, []);

  useEffect(() => {
    if (searchParams.get("action") === "new-repo") {
      openModal("init");
      setSearchParams({}, { replace: true });
    }
    // setSearchParams is a stable react-router reference
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  // Owns row-data updates (the numbers) for both the per-row and "Refresh All" buttons below.
  // Only "finished" is handled here — the failure marker (statsFailed) and spinner
  // (statsRefreshing) are both already derived by ActivityProvider from the same bus, so this
  // page doesn't need its own "failed" branch. refreshRepoStats already writes repo_stats_cache
  // before it emits "finished" (see repo.rs's fetch_and_cache_stats), so re-reading here is a
  // guaranteed cache hit — it never re-triggers a restic call or another task event.
  useEffect(() => {
    const unlistenTask = listen<TaskEvent>("task", (e) => {
      const t = e.payload;
      if (t.kind !== "stats" || t.phase !== "finished") return;
      getRepoStats(t.repoId)
        .then((s) => setStatsMap((prev) => ({ ...prev, [t.repoId]: s })))
        .catch(() => {});
    });
    return () => { unlistenTask.then((fn) => fn()); };
  }, []);

  // Fire-and-forget: display updates (spinner via statsRefreshing, data via the task listener
  // above) are entirely event-driven now, not chained off this promise.
  const refreshRow = (repo: Repository) => {
    refreshRepoStats(repo.id).catch(() => {});
  };

  useEffect(() => {
    indexAllTargetsRef.current = indexAllTargets;
  }, [indexAllTargets]);

  // Adopts a batch reported by getActiveIndexBatch as this modal's tracked state — shared by
  // handleIndexAll's "already running" paths (a fresh call rejected because one exists, or a
  // repo we're re-opening the modal for that already has one). `statusMap` is passed in rather
  // than read from state to avoid racing a just-fetched local value.
  const adoptIndexAllBatch = (status: ActiveIndexBatchStatus, statusMap: Record<string, string>) => {
    const completed = new Set(status.targetIds.filter((id) => statusMap[id] === "complete"));
    setIndexAllTargets(status.targetIds);
    setIndexAllCompleted(completed);
    setIndexAllFailedSet(new Set());
    setIndexAllBatchState({ kind: status.started ? "running" : "queued", operationId: status.operationId });
  };

  // Live progress while a batch is tracked — mirrors RepoSearchPage.tsx's own `task` listener,
  // scoped to whichever repo the modal currently targets.
  useEffect(() => {
    const repoId = indexAllRepo?.id;
    if (!repoId) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<TaskEvent>("task", (e) => {
      const t = e.payload;
      if (t.kind !== "index" || t.repoId !== repoId) return;
      // The batch-level op (no targetId) — tracks queued/running/idle for the Stop button and
      // progress header. Per-snapshot ops (targetId set) update the completed/failed sets below.
      if (!t.targetId) {
        if (t.origin !== "manual") return;
        if (t.phase === "pending") {
          setIndexAllBatchState({ kind: "queued", operationId: t.operationId });
        } else if (t.phase === "started") {
          setIndexAllBatchState({ kind: "running", operationId: t.operationId });
        } else if (t.phase === "finished" || t.phase === "failed" || t.phase === "cancelled") {
          setIndexAllBatchState({ kind: "idle" });
        }
        return;
      }
      if (t.phase !== "finished" && t.phase !== "failed") return;
      const snapshotId = t.targetId;
      if (!indexAllTargetsRef.current.includes(snapshotId)) return;
      if (t.phase === "finished") {
        setIndexAllCompleted((prev) => {
          if (prev.has(snapshotId)) return prev;
          const next = new Set(prev);
          next.add(snapshotId);
          return next;
        });
      } else {
        setIndexAllFailedSet((prev) => {
          if (prev.has(snapshotId)) return prev;
          const next = new Set(prev);
          next.add(snapshotId);
          return next;
        });
      }
    }).then((u) => {
      if (cancelled) { u(); return; }
      unlisten = u;
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [indexAllRepo?.id]);

  const handleIndexAll = async (repo: Repository) => {
    // Already tracking a running/queued batch for this exact repo — just reopen the modal
    // rather than restarting (index_snapshots_batch would reject a duplicate call anyway).
    if (indexAllRepo?.id === repo.id && indexAllBatchActive) {
      setIndexAllOpen(true);
      return;
    }
    setIndexAllRepo(repo);
    setIndexAllError("");
    setIndexAllStopped(false);
    setIndexAllNothingToDo(false);
    setIndexAllTargets([]);
    setIndexAllCompleted(new Set());
    setIndexAllFailedSet(new Set());
    setIndexAllOpen(true);
    setIndexAllBatchState({ kind: "starting" });
    try {
      const [snaps, statusMap] = await Promise.all([listSnapshots(repo.id), getSnapshotIndexStatus(repo.id)]);
      // A batch might already be running for this repo from elsewhere (e.g. its Search page) —
      // adopt it instead of starting a duplicate.
      const active = await getActiveIndexBatch(repo.id).catch(() => null);
      if (active) {
        adoptIndexAllBatch(active, statusMap);
        return;
      }
      const targets = snaps
        .filter((s) => statusMap[s.id] !== "complete" && statusMap[s.id] !== "in_progress")
        .map((s) => s.id);
      if (targets.length === 0) {
        setIndexAllBatchState({ kind: "idle" });
        setIndexAllNothingToDo(true);
        return;
      }
      setIndexAllTargets(targets);
      // Indexed sequentially, one snapshot at a time, by the backend — progress arrives via
      // the task listener above.
      await indexSnapshotsBatch(repo.id, targets);
    } catch (err) {
      if (String(err) === INDEX_BATCH_ALREADY_ACTIVE_ERROR) {
        try {
          const [status, statusMap] = await Promise.all([getActiveIndexBatch(repo.id), getSnapshotIndexStatus(repo.id)]);
          if (status) {
            adoptIndexAllBatch(status, statusMap);
            return;
          }
        } catch {
          // fall through to idle below
        }
        setIndexAllBatchState({ kind: "idle" });
        return;
      }
      setIndexAllError("Failed to start indexing.");
      setIndexAllBatchState({ kind: "idle" });
    }
  };

  const handleStopIndexAll = async () => {
    if (!indexAllOperationId) return;
    try {
      await cancelIndexBatch(indexAllOperationId);
      setIndexAllStopped(true);
    } catch {
      // best-effort; batch will keep running if this fails
    }
  };

  const handleRefreshRow = (e: MouseEvent, repo: Repository) => {
    e.stopPropagation();
    refreshRow(repo);
  };

  const handleRefreshAll = async () => {
    // Includes remote repos unconditionally — this is a manual, user-initiated action, and
    // stats never refresh on their own anymore, so there's no surprise-bandwidth risk to guard
    // against with remote_auto_refresh here (that setting still gates every *automatic* remote
    // activity: the cache warmer's snapshot/index sweep, SnapshotsPage's background refresh,
    // and Index All — see CLAUDE.md's Restic Integration section).
    // One repo at a time, not Promise.allSettled/parallel (as this was originally, and stayed,
    // since 67e48f4) — each `restic stats` call is a real subprocess with no cap, unlike
    // indexing (IndexHandle::gate limits that to one process app-wide after a prior RAM
    // incident — see CLAUDE.md's Intentional Designs). Firing one per repo simultaneously could
    // spike CPU/disk/network noticeably with several repos, especially alongside an "Index All"
    // batch also running. Each call's outcome is still picked up by the bus regardless of
    // ordering (finished → the page's own "task" listener refreshes statsMap; failed → sets
    // statsFailed via ActivityProvider), same as a single row refresh — the `.catch` here only
    // keeps the loop moving to the next repo if one call rejects, it doesn't drive any UI state.
    //
    // `current` is 0-indexed (the completed-so-far count, matching SnapshotsPage's
    // multiDeleteProgress/multiCopyProgress convention exactly — set to the loop index
    // *before* that item starts, with the render adding +1 to show "working on item N").
    // Wrapped in try/finally so this is guaranteed to clear even if something in the loop
    // throws unexpectedly (every awaited call already has its own `.catch`, so this is a
    // backstop, not the primary path) — the Activity panel has no other way to notice a
    // stuck batch the way a backend task would via OperationCtx's Drop.
    try {
      for (let i = 0; i < repos.length; i++) {
        setStatsRefreshAllProgress({ current: i, total: repos.length });
        await refreshRepoStats(repos[i].id).catch(() => {});
      }
    } finally {
      setStatsRefreshAllProgress(null);
    }
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!form.name || !form.path || (!noPassword && !form.password)) {
      setError("All fields are required.");
      return;
    }
    setLoading(true);
    setError("");
    const id = crypto.randomUUID();
    try {
      if (modalMode === "init") {
        await initRepo(id, form.name, form.path, form.password);
      } else {
        await addRepo(id, form.name, form.path, noPassword ? "" : form.password);
      }
      await load();
      setModalMode(null);
      setForm({ name: "", path: "", password: "" });
      setNoPassword(false);
      if (!isRemoteRepo(form.path)) {
        refreshSnapshots(id).catch(() => {});
        refreshRepoStats(id)
          .then((s) => setStatsMap((prev) => ({ ...prev, [id]: s })))
          .catch(() => {});
      }
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleRemove = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await removeRepo(deleteTarget.id);
      setDeleteTarget(null);
      await load();
    } finally {
      setDeleting(false);
    }
  };

  const handleRename = async (e: FormEvent) => {
    e.preventDefault();
    if (!editTarget || !editName.trim() || !editPath.trim() || (!editNoPassword && !editPassword.trim())) return;
    setRenaming(true);
    try {
      if (editName.trim() !== editTarget.name) {
        await renameRepo(editTarget.id, editName.trim());
      }
      if (editPath.trim() !== editTarget.path) {
        await updateRepoPath(editTarget.id, editPath.trim());
      }
      const newPassword = editNoPassword ? "" : editPassword.trim();
      const originalPassword = await getRepoPassword(editTarget.id);
      if (newPassword !== originalPassword) {
        await updateRepoPassword(editTarget.id, newPassword);
      }
      await load();
      setEditTarget(null);
    } finally {
      setRenaming(false);
    }
  };

  const openModal = (mode: ModalMode) => {
    setForm({ name: "", path: "", password: "" });
    setNoPassword(false);
    setError("");
    setTestResult(null);
    setPathMode("local");
    setModalMode(mode);
  };

  const handleTest = async () => {
    if (!form.path || (!noPassword && !form.password)) {
      setTestResult({ ok: false, message: "Path and password are required to test." });
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      await testRepoConnection(form.path, noPassword ? "" : form.password);
      setTestResult({ ok: true, message: "Connection successful — repository is accessible." });
    } catch (err: any) {
      setTestResult({ ok: false, message: String(err) });
    } finally {
      setTesting(false);
    }
  };

  const pickFolder = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected) setForm((f) => ({ ...f, path: selected as string }));
  };

  useEffect(() => {
    if (!mirroring) return;
    mirrorStartRef.current = Date.now();
    setMirrorElapsed(0);
    const id = setInterval(() => {
      setMirrorElapsed(Math.floor((Date.now() - mirrorStartRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [mirroring]);

  useEffect(() => {
    if (!pruning) return;
    pruneStartRef.current = Date.now();
    setPruneElapsed(0);
    const id = setInterval(() => {
      setPruneElapsed(Math.floor((Date.now() - pruneStartRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [pruning]);

  const handleMirror = async () => {
    if (!mirrorSource || !mirrorDestId) return;
    setMirroring(true);
    setMirrorDone(false);
    setMirrorCancelled(false);
    setMirrorError("");
    try {
      await mirrorRepo(mirrorSource.id, mirrorDestId);
      setMirrorDone(true);
      setStatsMap((prev) => { const next = { ...prev }; delete next[mirrorDestId]; return next; });
      refreshRepoStats(mirrorDestId)
        .then((s) => setStatsMap((prev) => ({ ...prev, [mirrorDestId]: s })))
        .catch(() => {});
    } catch (err: any) {
      const msg = String(err);
      if (msg === "cancelled") {
        setMirrorCancelled(true);
      } else {
        setMirrorError(msg);
      }
    } finally {
      setMirroring(false);
    }
  };

  const handleCancelMirror = async () => {
    try { await cancelMirror(); } catch {}
  };

  const closeMirrorModal = () => {
    if (mirroring) return;
    setMirrorSource(null);
    setMirrorDestId("");
    setMirrorDone(false);
    setMirrorCancelled(false);
    setMirrorError("");
  };

  const handlePrune = async () => {
    if (!pruneTarget) return;
    setPruning(true);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    try {
      await pruneRepo(pruneTarget.id);
      setPruneDone(true);
    } catch (err: any) {
      const msg = String(err);
      if (msg === "Cancelled") {
        setPruneCancelled(true);
      } else {
        setPruneError(msg);
      }
    } finally {
      setPruning(false);
    }
  };

  const handleCancelPrune = async () => {
    try { await cancelPrune(); } catch {}
  };

  // Hides the modal only — a still-running prune keeps going in the background and stays
  // visible/cancellable via the Activity panel's activePrune row (see activity.tsx). Reopening
  // via the "Prune…" context-menu item (below) shows the same repo's live progress rather than
  // a blank state, since pruneTarget is left untouched here. Mirrors SettingsPage's
  // closePruneModal for "Prune All Repositories".
  const closePruneModal = () => {
    setPruneModalOpen(false);
  };

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Repositories</h1>
          <p className="text-sm text-gray-500 mt-0.5">Manage your Restic backup repositories</p>
        </div>
        <div className="flex gap-2">
          {repos.length > 0 && (
            <Button
              variant="secondary"
              disabled={refreshingAll}
              onClick={handleRefreshAll}
            >
              {statsRefreshAllProgress
                ? `Refreshing… (${statsRefreshAllProgress.current + 1}/${statsRefreshAllProgress.total})`
                : "Refresh Stats"}
            </Button>
          )}
          <Button variant="secondary" onClick={() => openModal("add")}>
            Open Existing
          </Button>
          <Button onClick={() => openModal("init")}>
            + New Repository
          </Button>
        </div>
      </div>

      {repos.length === 0 ? (
        <EmptyState
          title="No repositories yet"
          description="Create a new repository or open an existing one."
          action={<Button onClick={() => openModal("init")}>+ New Repository</Button>}
          icon={
            <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1}
                d="M3 7a2 2 0 012-2h14a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" />
            </svg>
          }
        />
      ) : (
        <div className="grid gap-3">
          {repos.map((repo) => (
            <div
              key={repo.id}
              className="flex items-center justify-between p-4 rounded-xl border bg-gray-900 border-gray-800 hover:border-gray-700 transition-colors cursor-pointer"
              onClick={() => navigate(`/snapshots/${repo.id}`)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setContextMenu({ repo, x: e.clientX, y: e.clientY });
              }}
            >
              <div className="flex items-center gap-3">
                <div>
                  <p className="text-sm font-medium text-gray-100">{repo.name}</p>
                  <p className="text-xs text-gray-500 mt-0.5 font-mono">{repo.path}</p>
                </div>
              </div>
              <div className="flex items-center gap-4">
                <div className="flex items-center gap-2">
                  <div className="text-right min-w-[80px]">
                    {repo.id in statsMap ? (
                      statsMap[repo.id] ? (
                        <>
                          <p className="text-sm font-medium text-gray-300">{formatBytes(statsMap[repo.id]!.total_size)}</p>
                          <p className="text-xs text-gray-600">{statsMap[repo.id]!.snapshots_count} snapshot{statsMap[repo.id]!.snapshots_count !== 1 ? "s" : ""}</p>
                          {statsMap[repo.id]!.cached_at != null && (
                            <p
                              className="text-xs text-gray-600"
                              title={formatTimestamp(statsMap[repo.id]!.cached_at!)}
                            >
                              Refreshed {formatRelative(statsMap[repo.id]!.cached_at!)}
                            </p>
                          )}
                          {statsFailed.includes(repo.id) && (
                            <p className="text-xs text-red-400">refresh failed</p>
                          )}
                        </>
                      ) : (
                        <p className="text-xs text-gray-600">unavailable</p>
                      )
                    ) : isRemoteRepo(repo.path) ? (
                      <p className="text-xs text-gray-600">—</p>
                    ) : (
                      <p className="text-xs text-gray-600 animate-pulse">loading…</p>
                    )}
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={refreshingAll || statsRefreshing.includes(repo.id)}
                    onClick={(e) => handleRefreshRow(e, repo)}
                    className="text-gray-500 hover:text-blue-400"
                    title="Refresh stats"
                  >
                    <svg
                      className={`w-4 h-4 ${statsRefreshing.includes(repo.id) ? "animate-spin" : ""}`}
                      fill="none" stroke="currentColor" viewBox="0 0 24 24"
                    >
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                  </Button>
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setEditTarget(repo);
                      setEditName(repo.name);
                      setEditPath(repo.path);
                      setEditPassword("");
                      setEditNoPassword(false);
                      setEditTestResult(null);
                      getRepoPassword(repo.id)
                        .then((pw) => { setEditPassword(pw); setEditNoPassword(pw === ""); })
                        .catch(() => {});
                    }}
                    className="text-gray-500 hover:text-blue-400"
                    title="Rename"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                    </svg>
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setMirrorSource(repo);
                      setMirrorDestId("");
                      setMirrorDone(false);
                      setMirrorCancelled(false);
                      setMirrorError("");
                    }}
                    className="text-gray-500 hover:text-purple-400"
                    title="Mirror to another repository"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M8 7h12m0 0l-4-4m4 4l-4 4m0 6H4m0 0l4 4m-4-4l4-4" />
                    </svg>
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDeleteTarget(repo);
                    }}
                    className="text-gray-500 hover:text-red-300"
                    title="Remove"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                    </svg>
                  </Button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        title={`Check: ${checkRepoName}`}
        open={checking || checkResult !== null}
        onClose={() => { if (!checking) setCheckResult(null); }}
      >
        {checking ? (
          <div className="flex flex-col items-center justify-center py-10 gap-3 text-sm text-gray-400">
            <Spinner className="w-6 h-6 text-blue-400" />
            Checking repository…
          </div>
        ) : checkResult && (
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
        title="Remove Repository"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to remove{" "}
          <span className="font-semibold text-gray-50">{deleteTarget?.name}</span>?
          This only removes it from the list — the repository data on disk is not deleted.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={deleting}>
            Cancel
          </Button>
          <Button variant="danger" loading={deleting} onClick={handleRemove}>
            Remove
          </Button>
        </div>
      </Modal>

      <Modal
        title="Edit Repository"
        open={editTarget !== null}
        onClose={() => setEditTarget(null)}
      >
        <form onSubmit={handleRename} className="space-y-4">
          <Input
            label="Name"
            placeholder="e.g. Home Backup"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            autoFocus
          />
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-2">Path</label>
            {editTarget && isRemoteRepo(editTarget.path) ? (
              <input
                type="text"
                className="w-full px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono text-gray-300 focus:outline-none focus:border-blue-500 placeholder-gray-600"
                value={editPath}
                onChange={(e) => setEditPath(e.target.value)}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
              />
            ) : (
              <div className="flex items-center gap-2">
                <span className={`flex-1 px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono truncate min-w-0 ${editPath ? "text-gray-300" : "text-gray-600"}`}>
                  {editPath || "No folder selected"}
                </span>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={async () => {
                    const selected = await open({ directory: true, multiple: false });
                    if (selected) setEditPath(selected as string);
                  }}
                >
                  Browse…
                </Button>
              </div>
            )}
          </div>
          {editNoPassword ? (
            // Passwordless repo: read-only indicator. The password/passwordless
            // boundary can't be crossed here — converting a repo requires
            // re-init (not supported), and flipping a password repo to
            // passwordless would just store "" and brick it.
            <div className="flex flex-col gap-1">
              <label className="text-sm text-gray-400 font-medium">Password</label>
              <div className="w-full bg-gray-800 border border-gray-700 rounded-md px-3 py-2 text-sm italic text-gray-500">
                No Password (--insecure-no-password)
              </div>
            </div>
          ) : (
            <Input
              label="Password"
              type="password"
              placeholder="Repository password"
              value={editPassword}
              onChange={(e) => { setEditPassword(e.target.value); setEditTestResult(null); }}
            />
          )}
          {editTestResult && (
            <div className={`text-sm rounded-lg px-3 py-2 ${editTestResult.ok ? "bg-green-900/40 text-green-300 border border-green-700" : "bg-red-900/40 text-red-300 border border-red-700"}`}>
              {editTestResult.message}
            </div>
          )}
          <div className="flex items-center justify-between pt-2">
            <Button
              type="button"
              variant="secondary"
              loading={editTesting}
              onClick={async () => {
                if (!editPath.trim() || (!editNoPassword && !editPassword.trim())) {
                  setEditTestResult({ ok: false, message: "Path and password are required to test." });
                  return;
                }
                setEditTesting(true);
                setEditTestResult(null);
                try {
                  await testRepoConnection(editPath.trim(), editNoPassword ? "" : editPassword.trim());
                  setEditTestResult({ ok: true, message: "Connection successful — repository is accessible." });
                } catch (err: any) {
                  setEditTestResult({ ok: false, message: String(err) });
                } finally {
                  setEditTesting(false);
                }
              }}
            >
              Test Connection
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" type="button" onClick={() => setEditTarget(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={renaming}>
                Save
              </Button>
            </div>
          </div>
        </form>
      </Modal>

      <Modal
        title="Mirror Repository"
        open={mirrorSource !== null}
        onClose={closeMirrorModal}
      >
        {mirrorDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Mirror complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              All snapshots from <span className="font-semibold text-gray-50">{mirrorSource?.name}</span> have been copied to{" "}
              <span className="font-semibold text-gray-50">{repos.find((r) => r.id === mirrorDestId)?.name}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeMirrorModal}>Close</Button>
            </div>
          </>
        ) : mirrorCancelled ? (
          <>
            <p className="text-sm text-gray-400 mb-4">Mirror was cancelled.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeMirrorModal}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Copy all snapshots from <span className="font-semibold text-gray-50">{mirrorSource?.name}</span> into another
              repository. Snapshots that already exist in the destination are skipped.
            </p>
            <div className="mb-4">
              <label className="block text-sm font-medium text-gray-300 mb-2">Destination Repository</label>
              <div className="relative">
                <select
                  className="w-full appearance-none px-3 py-2 pr-8 rounded-lg bg-gray-800 border border-gray-700 text-sm text-gray-300 focus:outline-none focus:border-blue-500"
                  value={mirrorDestId}
                  onChange={(e) => setMirrorDestId(e.target.value)}
                  disabled={mirroring}
                >
                  <option value="">Select a repository…</option>
                  {repos
                    .filter((r) => r.id !== mirrorSource?.id)
                    .map((r) => (
                      <option key={r.id} value={r.id}>{r.name} — {r.path}</option>
                    ))}
                </select>
                <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">▾</div>
              </div>
            </div>
            {mirrorError && (
              <p className="text-sm text-red-300 mb-3">{mirrorError}</p>
            )}
            {mirroring && (
              <div className="mb-4">
                <p className="text-xs text-gray-400 mb-1">Copying snapshots…</p>
                <ProgressBar indeterminate colorClass="bg-purple-500" heightClass="h-2" />
              </div>
            )}
            <div className="flex items-center justify-between">
              {mirroring ? (
                <span className="text-xs text-gray-500">
                  {mirrorElapsed < 60
                    ? `${mirrorElapsed}s elapsed`
                    : `${Math.floor(mirrorElapsed / 60)}m ${mirrorElapsed % 60}s elapsed`}
                </span>
              ) : (
                <span />
              )}
              <div className="flex gap-2">
              {mirroring ? (
                <Button variant="secondary" onClick={handleCancelMirror}>Cancel</Button>
              ) : (
                <>
                  <Button variant="secondary" onClick={closeMirrorModal}>Cancel</Button>
                  <Button
                    onClick={handleMirror}
                    disabled={!mirrorDestId}
                    className="bg-purple-600 hover:bg-purple-500"
                  >
                    Mirror
                  </Button>
                </>
              )}
              </div>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title={`Prune: ${pruneTarget?.name ?? ""}`}
        open={pruneModalOpen}
        onClose={closePruneModal}
      >
        {pruneDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Prune complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Unreferenced data has been removed from{" "}
              <span className="font-semibold text-gray-50">{pruneTarget?.name}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </>
        ) : pruneCancelled ? (
          <>
            <p className="text-sm text-gray-400 mb-4">Prune was cancelled.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </>
        ) : pruning ? (
          <>
            <p className="text-xs text-gray-400 mb-1">Pruning repository…</p>
            <ProgressBar indeterminate heightClass="h-2" className="mb-4" />
            {pruneError && <p className="text-sm text-red-300 mb-3">{pruneError}</p>}
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </span>
              <div className="flex gap-2">
                <Button variant="secondary" onClick={closePruneModal} title="Keep pruning in the background">
                  Hide
                </Button>
                <Button variant="secondary" onClick={handleCancelPrune}>Cancel</Button>
              </div>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-5">
              Remove unreferenced data from{" "}
              <span className="font-semibold text-gray-50">{pruneTarget?.name}</span>? This frees disk space
              by deleting pack files that are no longer referenced by any snapshot.
            </p>
            {pruneError && <p className="text-sm text-red-300 mb-3">{pruneError}</p>}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closePruneModal}>Cancel</Button>
              <Button onClick={handlePrune}>Prune</Button>
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
              label: "Open Snapshots",
              onClick: () => navigate(`/snapshots/${contextMenu.repo.id}`),
            },
            {
              label: "Search Files…",
              onClick: () => navigate(`/snapshots/${contextMenu.repo.id}/search`),
            },
            {
              label: "Index All Snapshots",
              disabled: repoNeedsIndexing[contextMenu.repo.id] === false,
              onClick: () => handleIndexAll(contextMenu.repo),
            },
            { separator: true },
            {
              label: "Refresh Stats",
              disabled: refreshingAll || statsRefreshing.includes(contextMenu.repo.id),
              onClick: () => refreshRow(contextMenu.repo),
            },
            {
              label: "Check Repository",
              onClick: () => {
                const repo = contextMenu.repo;
                setCheckRepoName(repo.name);
                setCheckResult(null);
                setChecking(true);
                checkRepo(repo.id)
                  .then(setCheckResult)
                  .catch((err) => setCheckResult({ success: false, errors: [String(err)], duration_seconds: 0 }))
                  .finally(() => setChecking(false));
              },
            },
            {
              label: "Edit",
              onClick: () => {
                const repo = contextMenu.repo;
                setEditTarget(repo);
                setEditName(repo.name);
                setEditPath(repo.path);
                setEditPassword("");
                setEditNoPassword(false);
                setEditTestResult(null);
                getRepoPassword(repo.id)
                  .then((pw) => { setEditPassword(pw); setEditNoPassword(pw === ""); })
                  .catch(() => {});
              },
            },
            {
              label: "Mirror…",
              onClick: () => {
                setMirrorSource(contextMenu.repo);
                setMirrorDestId("");
                setMirrorDone(false);
                setMirrorCancelled(false);
                setMirrorError("");
              },
            },
            {
              label: "Prune…",
              // Prune is single-in-flight app-wide (PruneHandle's busy guard) — disable this
              // for every repo except the one currently pruning, so a click on a *different*
              // repo can't silently reopen the modal onto the wrong repo's live progress (the
              // reset-on-open logic below only skips resetting pruneTarget for that same repo).
              disabled: pruning && pruneTarget?.id !== contextMenu.repo.id,
              onClick: () => {
                // Only reset to a fresh confirm screen when nothing is running — a
                // still-running prune (survived a prior Hide) should reopen into its live
                // progress instead of a blank confirm screen.
                if (!pruning) {
                  setPruneTarget(contextMenu.repo);
                  setPruneDone(false);
                  setPruneCancelled(false);
                  setPruneError("");
                }
                setPruneModalOpen(true);
              },
            },
            { separator: true },
            {
              label: "Delete",
              variant: "danger",
              onClick: () => setDeleteTarget(contextMenu.repo),
            },
          ] satisfies ContextMenuItemDef[]}
        />
      )}

      <Modal
        title={modalMode === "init" ? "Create New Repository" : "Open Existing Repository"}
        open={modalMode !== null}
        onClose={() => setModalMode(null)}
      >
        <form onSubmit={handleSubmit} className="space-y-4">
          <Input
            label="Name"
            placeholder="e.g. Home Backup"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-2">
              Repository Location
            </label>
            <div className="flex rounded-lg overflow-hidden border border-gray-700 mb-3">
              <button
                type="button"
                onClick={() => { setPathMode("local"); setForm((f) => ({ ...f, path: "" })); setTestResult(null); }}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${pathMode === "local" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Local Path
              </button>
              <button
                type="button"
                onClick={() => { setPathMode("remote"); setForm((f) => ({ ...f, path: "" })); setTestResult(null); }}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${pathMode === "remote" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Remote URL
              </button>
            </div>
            {pathMode === "local" ? (
              <div className="flex items-center gap-2">
                <span className={`flex-1 px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono truncate min-w-0 ${form.path ? "text-gray-300" : "text-gray-600"}`}>
                  {form.path || "No folder selected"}
                </span>
                <Button type="button" variant="secondary" size="sm" onClick={pickFolder}>
                  Browse…
                </Button>
              </div>
            ) : (
              <input
                type="text"
                className="w-full px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono text-gray-300 focus:outline-none focus:border-blue-500 placeholder-gray-600"
                value={form.path}
                onChange={(e) => { setForm({ ...form, path: e.target.value }); setTestResult(null); }}
                placeholder="s3:s3.amazonaws.com/bucket or sftp:user@host:/path"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
                autoFocus
              />
            )}
          </div>
          <Input
            label="Password"
            type="password"
            placeholder="Repository password"
            value={form.password}
            onChange={(e) => { setForm({ ...form, password: e.target.value }); setTestResult(null); }}
            disabled={modalMode === "add" && noPassword}
          />
          {modalMode === "add" && (
            <label className="flex items-center gap-2 text-sm text-gray-400">
              <input
                type="checkbox"
                checked={noPassword}
                onChange={(e) => {
                  setNoPassword(e.target.checked);
                  setForm((f) => ({ ...f, password: "" }));
                  setTestResult(null);
                }}
              />
              No Password (--insecure-no-password)
            </label>
          )}
          {testResult && (
            <div className={`text-sm rounded-lg px-3 py-2 ${testResult.ok ? "bg-green-900/40 text-green-300 border border-green-700" : "bg-red-900/40 text-red-300 border border-red-700"}`}>
              {testResult.message}
            </div>
          )}
          {error && <p className="text-sm text-red-300">{error}</p>}
          <div className="flex items-center justify-between pt-2">
            {modalMode === "add" && (
              <Button type="button" variant="secondary" loading={testing} onClick={handleTest}>
                Test Connection
              </Button>
            )}
            <div className="flex gap-2 ml-auto">
              <Button variant="secondary" type="button" onClick={() => setModalMode(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={loading}>
                {modalMode === "init" ? "Create" : "Open"}
              </Button>
            </div>
          </div>
        </form>
      </Modal>

      <Modal
        title="Index All Snapshots"
        open={indexAllOpen}
        onClose={() => setIndexAllOpen(false)}
      >
        {indexAllError ? (
          <div className="py-2">
            <p className="text-sm text-red-300 mb-4">{indexAllError}</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : indexAllNothingToDo ? (
          <div className="py-2">
            <p className="text-sm text-gray-300 mb-4">All snapshots are already indexed.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : indexAllStopped && indexAllDoneCount < indexAllTotal ? (
          <div className="py-2">
            <p className="text-sm text-gray-300 mb-3">
              Stopped after {indexAllDoneCount} of {indexAllTotal} snapshots.
            </p>
            <p className="text-xs text-gray-600 mb-4">
              The remaining snapshots were not indexed. You can resume later from this repository.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : indexAllDoneCount < indexAllTotal || indexAllTotal === 0 ? (
          <div className="py-2">
            {indexAllQueued || indexAllTotal === 0 ? (
              <>
                <p className="text-sm text-gray-300 mb-3">
                  {indexAllQueued ? "Queued — waiting for other indexing to finish…" : "Starting…"}
                </p>
                <p className="text-xs text-gray-600 mb-4">
                  Only one repository is indexed at a time. This batch will start automatically
                  as soon as the one ahead of it finishes.
                </p>
              </>
            ) : (
              <>
                <p className="text-sm text-gray-300 mb-3">
                  Indexing {indexAllDoneCount} of {indexAllTotal} snapshots…
                </p>
                <div className="w-full bg-gray-800 rounded-full h-1.5 mb-4">
                  <div
                    className="bg-blue-500 h-1.5 rounded-full transition-all"
                    style={{ width: `${(indexAllDoneCount / indexAllTotal) * 100}%` }}
                  />
                </div>
                <p className="text-xs text-gray-600 mb-4">
                  Snapshots are indexed one at a time. You can close this and keep browsing —
                  indexing continues in the background.
                </p>
              </>
            )}
            <div className="flex justify-end gap-2">
              <Button
                variant="secondary"
                onClick={handleStopIndexAll}
                disabled={!indexAllOperationId}
                title={indexAllOperationId ? undefined : "Starting…"}
              >
                Stop
              </Button>
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : (
          <div className="py-2">
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Indexing complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Indexed {indexAllTotal - indexAllFailedCount} of {indexAllTotal} snapshot{indexAllTotal !== 1 ? "s" : ""}.
              {indexAllFailedCount > 0 && ` ${indexAllFailedCount} failed and can be retried individually from the Snapshots page.`}
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
