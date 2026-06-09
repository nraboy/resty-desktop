import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { forgetByPlan, listBackupPlans, listRepos, removeBackupPlan, runBackup } from "../lib/invoke";
import type { BackupPlan, BackupProgress, Repository } from "../lib/types";
import Button from "../components/Button";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

function formatSeconds(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

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
  const [backupError, setBackupError] = useState("");
  const [backupDone, setBackupDone] = useState(false);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const unlistenRef = useRef<(() => void) | null>(null);

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

  useEffect(() => { load(); }, []);

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

  const openBackupModal = (plan: BackupPlan) => {
    setBackupPlan(plan);
    setBackupError("");
    setBackupDone(false);
    setProgress(null);
  };

  const closeBackupModal = () => {
    if (backupRunning) return;
    unlistenRef.current?.();
    unlistenRef.current = null;
    setBackupPlan(null);
  };

  const startBackup = async () => {
    if (!backupPlan) return;
    setBackupRunning(true);
    setBackupError("");
    setBackupDone(false);
    setProgress(null);

    const unlisten = await listen<BackupProgress>("backup:progress", (event) => {
      setProgress(event.payload);
    });
    unlistenRef.current = unlisten;

    try {
      await runBackup(backupPlan.repoId, backupPlan.paths, backupPlan.tags, backupPlan.excludes, backupPlan.id);
      if (backupPlan.retention) {
        try {
          await forgetByPlan(backupPlan.repoId, backupPlan.tags, backupPlan.paths, backupPlan.retention);
        } catch (pruneErr: any) {
          setBackupError("Backup succeeded but pruning failed: " + String(pruneErr));
          return;
        }
      }
      setBackupDone(true);
    } catch (err: any) {
      setBackupError(String(err));
    } finally {
      unlisten();
      unlistenRef.current = null;
      setBackupRunning(false);
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
        <Button onClick={() => navigate("/backup-plans/new")}>New Plan</Button>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
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
              className="bg-gray-900 border border-gray-800 rounded-xl px-5 py-4 flex items-center gap-4"
            >
              <div
                className="flex-1 min-w-0 cursor-pointer"
                onClick={() => navigate(`/backup-plans/${plan.id}`)}
              >
                <p className="text-sm font-medium text-gray-100 truncate">{plan.name}</p>
                <p className="text-xs text-gray-500 mt-0.5 truncate">
                  {repoName(plan.repoId)} &middot;{" "}
                  {plan.paths.length} {plan.paths.length === 1 ? "path" : "paths"}
                  {plan.tags.length > 0 && ` · ${plan.tags.join(", ")}`}
                </p>
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                <Button variant="secondary" size="sm" onClick={() => navigate(`/backup-plans/${plan.id}`)}>
                  Edit
                </Button>
                <Button size="sm" onClick={() => openBackupModal(plan)}>Backup Now</Button>
                <button
                  onClick={() => setDeleteTarget(plan)}
                  className="text-gray-500 hover:text-red-400 transition-colors text-xs px-2 py-1 rounded hover:bg-red-900/20"
                >
                  Delete
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        title="Delete Backup Plan"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-white">{deleteTarget?.name}</span>?
          This only removes the plan definition — existing snapshots are not affected.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={deleting}>Cancel</Button>
          <Button variant="danger" loading={deleting} onClick={handleDelete}>Delete</Button>
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

            {backupRunning && (
              <div className="space-y-2">
                <div className="flex justify-between text-xs text-gray-400">
                  <span>
                    {progress
                      ? `${progress.filesDone.toLocaleString()} / ${progress.totalFiles.toLocaleString()} files`
                      : "Starting…"}
                  </span>
                  <span>
                    {progress && progress.secondsRemaining != null
                      ? `~${formatSeconds(progress.secondsRemaining)} remaining`
                      : progress
                      ? `${formatSeconds(progress.secondsElapsed)} elapsed`
                      : ""}
                  </span>
                </div>
                <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                  <div
                    className="bg-blue-500 h-2 rounded-full transition-all duration-500"
                    style={{ width: `${((progress?.percentDone ?? 0) * 100).toFixed(1)}%` }}
                  />
                </div>
                {progress && progress.currentFiles.length > 0 && (
                  <p className="text-xs text-gray-500 font-mono truncate" title={progress.currentFiles[0]}>
                    {progress.currentFiles[0]}
                  </p>
                )}
              </div>
            )}

            {backupError && (
              <div className="p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 font-mono whitespace-pre-wrap">
                {backupError}
              </div>
            )}
            {backupDone && (
              <div className="p-3 bg-green-900/30 border border-green-700 rounded-lg text-sm text-green-300">
                Backup completed successfully.
              </div>
            )}

            <div className="flex justify-end gap-2 pt-1">
              {backupDone || backupError ? (
                <Button variant="secondary" onClick={closeBackupModal}>Close</Button>
              ) : (
                <>
                  <Button variant="secondary" onClick={closeBackupModal} disabled={backupRunning}>Cancel</Button>
                  <Button onClick={startBackup} loading={backupRunning}>
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
