import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { cancelBackup, checkFullDiskAccess, forgetByPlan, listBackupPlans, listRepos, removeBackupPlan, runBackup } from "../lib/invoke";
import type { FullDiskAccessStatus } from "../lib/invoke";
import type { BackupPlan, BackupProgress, Repository } from "../lib/types";
import { formatDuration } from "../lib/format";
import { needsFullDiskAccess } from "../lib/utils";
import Button from "../components/Button";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";
import ContextMenu, { type ContextMenuItemDef } from "../components/ContextMenu";

export default function BackupPlansPage() {
  const navigate = useNavigate();
  const [plans, setPlans] = useState<BackupPlan[]>([]);
  const [repos, setRepos] = useState<Repository[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  const [deleteTarget, setDeleteTarget] = useState<BackupPlan | null>(null);
  const [deleting, setDeleting] = useState(false);

  const [backupPlan, setBackupPlan] = useState<BackupPlan | null>(null);
  const [backupRunning, setBackupRunning] = useState(false);
  const [backupCancelling, setBackupCancelling] = useState(false);
  const [backupError, setBackupError] = useState("");
  const [backupDone, setBackupDone] = useState(false);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [applyingRetention, setApplyingRetention] = useState(false);
  const [elapsedSeconds, setElapsedSeconds] = useState(0);
  const unlistenRef = useRef<(() => void) | null>(null);
  const elapsedTimerRef = useRef<number | null>(null);

  const [contextMenu, setContextMenu] = useState<{ plan: BackupPlan; x: number; y: number } | null>(null);

  const [retentionPlan, setRetentionPlan] = useState<BackupPlan | null>(null);
  const [retentionRunning, setRetentionRunning] = useState(false);
  const [retentionError, setRetentionError] = useState("");
  const [retentionDone, setRetentionDone] = useState(false);

  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);
  const [fdaWarningPlan, setFdaWarningPlan] = useState<BackupPlan | null>(null);

  // Multi-select state
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // Multi-delete state
  const [multiDeleteOpen, setMultiDeleteOpen] = useState(false);
  const [multiDeleting, setMultiDeleting] = useState(false);
  const [multiDeleteProgress, setMultiDeleteProgress] = useState({ current: 0, total: 0 });
  const [multiDeleteError, setMultiDeleteError] = useState("");

  const selectAllCheckboxRef = useRef<HTMLInputElement>(null);

  const load = async () => {
    setLoading(true);
    try {
      const [p, r] = await Promise.all([listBackupPlans(), listRepos()]);
      setPlans(p);
      setRepos(r);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
  }, []);

  const repoName = (repoId: string) =>
    repos.find((r) => r.id === repoId)?.name ?? "Unknown repository";

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await removeBackupPlan(deleteTarget.id);
      setDeleteTarget(null);
      await load();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setDeleting(false);
    }
  };

  const exitSelectMode = () => {
    setSelectMode(false);
    setSelectedIds(new Set());
  };

  const toggleSelectOne = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const allSelected = plans.length > 0 && plans.every((p) => selectedIds.has(p.id));
  const someSelected = plans.some((p) => selectedIds.has(p.id));

  useEffect(() => {
    if (selectAllCheckboxRef.current) {
      selectAllCheckboxRef.current.indeterminate = someSelected && !allSelected;
    }
  }, [someSelected, allSelected]);

  const toggleSelectAll = () => {
    if (allSelected) {
      setSelectedIds(new Set());
    } else {
      setSelectedIds(new Set(plans.map((p) => p.id)));
    }
  };

  const handleMultiDelete = async () => {
    const ids = Array.from(selectedIds);
    setMultiDeleting(true);
    setMultiDeleteError("");
    setMultiDeleteProgress({ current: 0, total: ids.length });
    for (let i = 0; i < ids.length; i++) {
      setMultiDeleteProgress({ current: i, total: ids.length });
      try {
        await removeBackupPlan(ids[i]);
      } catch (err: any) {
        setMultiDeleteError(String(err));
        setMultiDeleting(false);
        await load();
        return;
      }
    }
    setMultiDeleting(false);
    setMultiDeleteOpen(false);
    exitSelectMode();
    await load();
  };

  const hasRetentionRules = (plan: BackupPlan) => {
    const r = plan.retention;
    return r && (r.keepLast != null || r.keepDaily != null || r.keepWeekly != null || r.keepMonthly != null || r.keepYearly != null);
  };

  const openRetentionModal = (plan: BackupPlan) => {
    setRetentionPlan(plan);
    setRetentionError("");
    setRetentionDone(false);
  };

  const closeRetentionModal = () => {
    if (retentionRunning) return;
    setRetentionPlan(null);
  };

  const startRetention = async () => {
    if (!retentionPlan || !retentionPlan.retention) return;
    setRetentionRunning(true);
    setRetentionError("");
    setRetentionDone(false);
    try {
      await forgetByPlan(retentionPlan.repoId, retentionPlan.tags, retentionPlan.paths, retentionPlan.retention);
      setRetentionDone(true);
    } catch (err: any) {
      setRetentionError(String(err));
    } finally {
      setRetentionRunning(false);
    }
  };

  const openBackupModal = (plan: BackupPlan) => {
    if (
      fdaStatus?.supported &&
      !fdaStatus.granted &&
      plan.paths.some(needsFullDiskAccess)
    ) {
      setFdaWarningPlan(plan);
      return;
    }
    setBackupPlan(plan);
    setBackupError("");
    setBackupDone(false);
    setProgress(null);
  };

  const proceedAfterFdaWarning = () => {
    const plan = fdaWarningPlan;
    setFdaWarningPlan(null);
    if (plan) {
      setBackupPlan(plan);
      setBackupError("");
      setBackupDone(false);
      setProgress(null);
    }
  };

  const stopElapsedTimer = () => {
    if (elapsedTimerRef.current != null) {
      clearInterval(elapsedTimerRef.current);
      elapsedTimerRef.current = null;
    }
  };

  // Clear the elapsed timer if the page unmounts mid-backup.
  useEffect(() => stopElapsedTimer, []);

  const closeBackupModal = () => {
    if (backupRunning) return;
    unlistenRef.current?.();
    unlistenRef.current = null;
    setBackupPlan(null);
  };

  const startBackup = async () => {
    if (!backupPlan) return;
    setBackupRunning(true);
    setBackupCancelling(false);
    setBackupError("");
    setBackupDone(false);
    setProgress(null);
    setApplyingRetention(false);

    // Wall-clock elapsed timer: runs until backup *and* retention finish, since
    // restic's reported secondsElapsed freezes once status lines stop streaming.
    const startedAt = Date.now();
    setElapsedSeconds(0);
    stopElapsedTimer();
    elapsedTimerRef.current = window.setInterval(() => {
      setElapsedSeconds(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);

    const unlisten = await listen<BackupProgress>("backup:progress", (event) => {
      setProgress(event.payload);
    });
    unlistenRef.current = unlisten;

    try {
      await runBackup(backupPlan.repoId, backupPlan.paths, backupPlan.tags, backupPlan.excludes, backupPlan.id, backupPlan.limitUpload, backupPlan.limitDownload);
      if (backupPlan.retention) {
        setApplyingRetention(true);
        try {
          await forgetByPlan(backupPlan.repoId, backupPlan.tags, backupPlan.paths, backupPlan.retention);
        } catch (pruneErr: any) {
          setBackupError("Backup succeeded but pruning failed: " + String(pruneErr));
          return;
        } finally {
          setApplyingRetention(false);
        }
      }
      setBackupDone(true);
    } catch (err: any) {
      const msg = String(err);
      if (!msg.includes("cancelled")) {
        setBackupError(msg);
      }
    } finally {
      stopElapsedTimer();
      unlisten();
      unlistenRef.current = null;
      setBackupRunning(false);
      setBackupCancelling(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-gray-500 text-sm">Loading backup plans…</p>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Backup Plans</h1>
          <p className="text-sm text-gray-500 mt-0.5">Define and run backup configurations.</p>
        </div>
        <div className="flex items-center gap-3">
          {selectMode ? (
            <Button variant="secondary" onClick={exitSelectMode}>
              Cancel select
            </Button>
          ) : (
            <>
              {plans.length > 0 && (
                <Button variant="secondary" onClick={() => setSelectMode(true)}>
                  Select Multiple
                </Button>
              )}
              <Button onClick={() => navigate("/backup-plans/new")}>New Plan</Button>
            </>
          )}
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {selectMode && (
        <div className="mb-4 p-3 bg-amber-900/20 border border-amber-700/50 rounded-lg text-sm text-amber-300 flex items-center justify-between gap-2">
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              ref={selectAllCheckboxRef}
              type="checkbox"
              checked={allSelected}
              onChange={toggleSelectAll}
              className="rounded bg-gray-700 border-gray-600"
            />
            <span>
              {selectedIds.size > 0
                ? `${selectedIds.size} plan${selectedIds.size !== 1 ? "s" : ""} selected`
                : "Select all"}
            </span>
          </label>
          <Button
            variant="danger"
            disabled={selectedIds.size === 0}
            onClick={() => { setMultiDeleteOpen(true); setMultiDeleteError(""); }}
          >
            Delete selected
          </Button>
        </div>
      )}

      {plans.length === 0 ? (
        <EmptyState
          title="No backup plans"
          description="Create a backup plan to define what to back up and where."
          action={<Button onClick={() => navigate("/backup-plans/new")}>Create a Plan</Button>}
        />
      ) : (
        <div className="space-y-3">
          {plans.map((plan) => (
            <div
              key={plan.id}
              className={`bg-gray-900 border border-gray-800 rounded-xl px-5 py-4 flex items-center gap-4 ${selectMode && selectedIds.has(plan.id) ? "ring-1 ring-blue-500/50" : ""}`}
              onContextMenu={(e) => {
                if (selectMode) return;
                e.preventDefault();
                e.stopPropagation();
                setContextMenu({ plan, x: e.clientX, y: e.clientY });
              }}
            >
              {selectMode && (
                <input
                  type="checkbox"
                  checked={selectedIds.has(plan.id)}
                  onChange={() => toggleSelectOne(plan.id)}
                  className="rounded bg-gray-700 border-gray-600 flex-shrink-0"
                />
              )}
              <div
                className="flex-1 min-w-0 cursor-pointer"
                onClick={() => selectMode ? toggleSelectOne(plan.id) : navigate(`/backup-plans/${plan.id}`)}
              >
                <p className="text-sm font-medium text-gray-100 truncate">{plan.name}</p>
                <p className="text-xs text-gray-500 mt-0.5 truncate">
                  {repoName(plan.repoId)} &middot;{" "}
                  {plan.paths.length} {plan.paths.length === 1 ? "path" : "paths"}
                  {(() => {
                    const excCount = plan.excludes.filter(e => e.trim() && !e.trim().startsWith('#')).length;
                    return excCount > 0 ? ` · ${excCount} ${excCount === 1 ? "exclusion" : "exclusions"}` : null;
                  })()}
                  {plan.tags.length > 0 && ` · ${plan.tags.join(", ")}`}
                </p>
                {plan.retention && (() => {
                  const r = plan.retention;
                  const parts: string[] = [];
                  if (r.keepLast != null) parts.push(`last ${r.keepLast}`);
                  if (r.keepDaily != null) parts.push(`${r.keepDaily}d`);
                  if (r.keepWeekly != null) parts.push(`${r.keepWeekly}w`);
                  if (r.keepMonthly != null) parts.push(`${r.keepMonthly}mo`);
                  if (r.keepYearly != null) parts.push(`${r.keepYearly}y`);
                  return parts.length > 0 ? (
                    <p className="text-xs text-gray-600 mt-0.5 truncate">
                      Retention: {parts.join(" · ")}
                    </p>
                  ) : null;
                })()}
              </div>
              {!selectMode && (
              <div className="flex items-center gap-2 flex-shrink-0">
                <Button
                  variant="ghost"
                  size="sm"
                  title="Edit plan"
                  onClick={() => navigate(`/backup-plans/${plan.id}`)}
                  className="text-gray-500 hover:text-blue-400"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                  </svg>
                </Button>
                {hasRetentionRules(plan) && (
                  <Button
                    variant="ghost"
                    size="sm"
                    title="Apply retention policy"
                    onClick={() => openRetentionModal(plan)}
                    className="text-gray-500 hover:text-yellow-400"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M3 4a1 1 0 011-1h16a1 1 0 011 1v2.586a1 1 0 01-.293.707l-6.414 6.414a1 1 0 00-.293.707V17l-4 4v-6.586a1 1 0 00-.293-.707L3.293 7.293A1 1 0 013 6.586V4z" />
                    </svg>
                  </Button>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  title="Run backup now"
                  onClick={() => openBackupModal(plan)}
                  className="text-gray-500 hover:text-green-400"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M14.752 11.168l-3.197-2.132A1 1 0 0010 9.87v4.263a1 1 0 001.555.832l3.197-2.132a1 1 0 000-1.664zM21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                  </svg>
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  title="Delete plan"
                  onClick={() => setDeleteTarget(plan)}
                  className="text-gray-500 hover:text-red-300"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                  </svg>
                </Button>
              </div>
              )}
            </div>
          ))}
        </div>
      )}

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Edit Plan",
              onClick: () => navigate(`/backup-plans/${contextMenu.plan.id}`),
            },
            { separator: true },
            {
              label: "Run Backup",
              onClick: () => openBackupModal(contextMenu.plan),
            },
            ...(hasRetentionRules(contextMenu.plan)
              ? [{ label: "Apply Retention Rules", onClick: () => openRetentionModal(contextMenu.plan) }]
              : []),
            { separator: true },
            {
              label: "Delete",
              variant: "danger" as const,
              onClick: () => setDeleteTarget(contextMenu.plan),
            },
          ] satisfies ContextMenuItemDef[]}
        />
      )}

      <Modal
        title={retentionPlan ? `Apply Retention: ${retentionPlan.name}` : "Apply Retention"}
        open={!!retentionPlan}
        onClose={closeRetentionModal}
      >
        {retentionPlan && (
          <div className="space-y-3">
            <div className="text-sm space-y-1">
              <div className="flex justify-between">
                <span className="text-gray-500">Repository</span>
                <span className="text-gray-200">{repoName(retentionPlan.repoId)}</span>
              </div>
              {retentionPlan.retention && (
                <div className="flex justify-between items-start pt-1 border-t border-gray-800 mt-1">
                  <span className="text-gray-500">Retention</span>
                  <span className="text-gray-200 text-right space-y-0.5">
                    {retentionPlan.retention.keepLast != null && <div>keep last {retentionPlan.retention.keepLast}</div>}
                    {retentionPlan.retention.keepDaily != null && <div>keep {retentionPlan.retention.keepDaily} daily</div>}
                    {retentionPlan.retention.keepWeekly != null && <div>keep {retentionPlan.retention.keepWeekly} weekly</div>}
                    {retentionPlan.retention.keepMonthly != null && <div>keep {retentionPlan.retention.keepMonthly} monthly</div>}
                    {retentionPlan.retention.keepYearly != null && <div>keep {retentionPlan.retention.keepYearly} yearly</div>}
                  </span>
                </div>
              )}
            </div>

            <p className="text-xs text-gray-500">
              Runs <code className="text-gray-400">restic forget --prune</code> with the plan's retention rules.
              Snapshots outside the policy will be permanently removed.
            </p>

            <div className="min-h-[48px] flex flex-col justify-center">
              {retentionRunning && (
                <div className="flex items-center gap-2 text-sm text-gray-400">
                  <svg className="animate-spin w-4 h-4 text-blue-400 flex-shrink-0" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                    <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                    <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
                  </svg>
                  Applying retention policy…
                </div>
              )}
              {retentionError && (
                <div className="p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 font-mono whitespace-pre-wrap break-all">
                  {retentionError}
                </div>
              )}
              {retentionDone && (
                <div className="p-3 bg-green-900/30 border border-green-700 rounded-lg text-sm text-green-300">
                  Retention policy applied successfully.
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 pt-1">
              {retentionDone || retentionError ? (
                <Button variant="secondary" onClick={closeRetentionModal}>Close</Button>
              ) : (
                <>
                  <Button variant="secondary" onClick={closeRetentionModal} disabled={retentionRunning}>Cancel</Button>
                  <Button onClick={startRetention} loading={retentionRunning} disabled={retentionRunning}>
                    {retentionRunning ? "Running…" : "Apply Retention"}
                  </Button>
                </>
              )}
            </div>
          </div>
        )}
      </Modal>

      <Modal
        title="Delete Backup Plan"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-gray-50">{deleteTarget?.name}</span>?
          This only removes the plan definition — existing snapshots are not affected.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={deleting}>Cancel</Button>
          <Button variant="danger" loading={deleting} onClick={handleDelete}>Delete</Button>
        </div>
      </Modal>

      {/* Multi-delete modal */}
      <Modal
        title="Delete Backup Plans"
        open={multiDeleteOpen}
        onClose={() => { if (!multiDeleting) { setMultiDeleteOpen(false); setMultiDeleteError(""); } }}
      >
        {multiDeleteError ? (
          <>
            <p className="text-sm text-red-300 mb-4">{multiDeleteError}</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => { setMultiDeleteOpen(false); setMultiDeleteError(""); }}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Delete {selectedIds.size} backup plan{selectedIds.size !== 1 ? "s" : ""}?
              This only removes the plan definitions — existing snapshots are not affected.
            </p>
            {multiDeleting && (
              <div className="mb-4">
                <div className="flex justify-between text-xs text-gray-400 mb-1">
                  <span>Deleting plan {multiDeleteProgress.current + 1} of {multiDeleteProgress.total}…</span>
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
                Delete {selectedIds.size} plan{selectedIds.size !== 1 ? "s" : ""}
              </Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Full Disk Access Not Detected"
        open={!!fdaWarningPlan}
        onClose={() => setFdaWarningPlan(null)}
      >
        <div className="space-y-4">
          <div className="flex items-start gap-3 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg">
            <svg className="w-5 h-5 text-amber-400 flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
            </svg>
            <p className="text-sm text-amber-300">
              This plan includes protected paths (e.g. <code className="text-amber-100">~/Library</code>), but Full Disk Access isn't granted — the backup may skip or fail on those paths.
            </p>
          </div>
          <p className="text-sm text-gray-400">
            To fix this, grant Full Disk Access to Resty Desktop in{" "}
            <span className="text-gray-200 font-medium">System Settings → Privacy &amp; Security → Full Disk Access</span>,
            then relaunch the app. macOS requires a relaunch for the new permission to take effect.
          </p>
          <div className="flex justify-end gap-2 pt-1">
            <Button variant="secondary" onClick={() => setFdaWarningPlan(null)}>Cancel</Button>
            <Button variant="secondary" onClick={proceedAfterFdaWarning}>Run Anyway</Button>
          </div>
        </div>
      </Modal>

      <Modal
        title={backupPlan ? `Backup: ${backupPlan.name}` : "Backup"}
        open={!!backupPlan}
        onClose={closeBackupModal}
      >
        {backupPlan && (
          <div className="space-y-3">
            <div className="text-sm space-y-1">
              <div className="flex justify-between">
                <span className="text-gray-500">Repository</span>
                <span className="text-gray-200">{repoName(backupPlan.repoId)}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-gray-500">Paths</span>
                <span className="text-gray-200">{backupPlan.paths.length} {backupPlan.paths.length === 1 ? "path" : "paths"}</span>
              </div>
              {backupPlan.excludes.filter(e => e.trim() && !e.trim().startsWith('#')).length > 0 && (
                <div className="flex justify-between">
                  <span className="text-gray-500">Exclusions</span>
                  <span className="text-gray-200">{backupPlan.excludes.filter(e => e.trim() && !e.trim().startsWith('#')).length} rules</span>
                </div>
              )}
              {backupPlan.retention && (
                <div className="flex justify-between items-start pt-1 border-t border-gray-800 mt-1">
                  <span className="text-gray-500">Retention</span>
                  <span className="text-gray-200 text-right space-y-0.5">
                    {backupPlan.retention.keepLast != null && <div>keep last {backupPlan.retention.keepLast}</div>}
                    {backupPlan.retention.keepDaily != null && <div>keep {backupPlan.retention.keepDaily} daily</div>}
                    {backupPlan.retention.keepWeekly != null && <div>keep {backupPlan.retention.keepWeekly} weekly</div>}
                    {backupPlan.retention.keepMonthly != null && <div>keep {backupPlan.retention.keepMonthly} monthly</div>}
                    {backupPlan.retention.keepYearly != null && <div>keep {backupPlan.retention.keepYearly} yearly</div>}
                  </span>
                </div>
              )}
            </div>

            <div className="min-h-[76px] flex flex-col justify-center">
              {backupRunning && (
                <div className="space-y-2">
                  <div className="flex justify-between text-xs text-gray-400">
                    <span>
                      {progress
                        ? `${progress.filesDone.toLocaleString()} / ${progress.totalFiles.toLocaleString()} files`
                        : "Starting…"}
                    </span>
                    <span>
                      {!applyingRetention && progress && progress.secondsRemaining != null
                        ? `~${formatDuration(progress.secondsRemaining)} remaining`
                        : `${formatDuration(elapsedSeconds)} elapsed`}
                    </span>
                  </div>
                  <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                    <div
                      className="bg-blue-500 h-2 rounded-full transition-all duration-500"
                      style={{ width: `${((progress?.percentDone ?? 0) * 100).toFixed(1)}%` }}
                    />
                  </div>
                  {applyingRetention ? (
                    <p className="text-xs text-gray-400">Applying retention rules…</p>
                  ) : (
                    <p className="text-xs text-gray-500 font-mono truncate" title={progress?.currentFiles[0] ?? ""}>
                      {progress?.currentFiles[0] ?? "Backing up…"}
                    </p>
                  )}
                </div>
              )}

              {backupError && (
                <div className="p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 font-mono whitespace-pre-wrap break-all">
                  {backupError}
                </div>
              )}
              {backupDone && (
                <div className="p-3 bg-green-900/30 border border-green-700 rounded-lg text-sm text-green-300">
                  Backup completed successfully.
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 pt-1">
              {backupDone || backupError ? (
                <Button variant="secondary" onClick={closeBackupModal}>Close</Button>
              ) : (
                <>
                  {backupRunning ? (
                    <Button
                      variant="danger"
                      disabled={backupCancelling}
                      onClick={async () => {
                        setBackupCancelling(true);
                        await cancelBackup();
                      }}
                    >
                      {backupCancelling ? "Stopping…" : "Stop Backup"}
                    </Button>
                  ) : (
                    <Button variant="secondary" onClick={closeBackupModal}>Cancel</Button>
                  )}
                  <Button onClick={startBackup} loading={backupRunning} disabled={backupRunning}>
                    {backupRunning ? "Running…" : "Start Backup"}
                  </Button>
                </>
              )}
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
