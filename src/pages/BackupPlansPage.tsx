import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { cancelBackup, forgetByPlan, listBackupPlans, listRepos, removeBackupPlan, runBackup } from "../lib/invoke";
import type { BackupPlan, BackupProgress, Repository } from "../lib/types";
import { formatDuration } from "../lib/format";
import Button from "../components/Button";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

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
    setBackupCancelling(false);
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
      const msg = String(err);
      if (!msg.includes("cancelled")) {
        setBackupError(msg);
      }
    } finally {
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
                <Button
                  variant="ghost"
                  size="sm"
                  title="Edit plan"
                  onClick={() => navigate(`/backup-plans/${plan.id}`)}
                  className="text-gray-500 hover:text-blue-400"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                    <path d="M2.695 14.763l-1.262 3.154a.5.5 0 00.65.65l3.155-1.262a4 4 0 001.343-.885L17.5 5.5a2.121 2.121 0 00-3-3L3.58 13.42a4 4 0 00-.885 1.343z" />
                  </svg>
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  title="Run backup now"
                  onClick={() => openBackupModal(plan)}
                  className="text-gray-500 hover:text-green-400"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                    <path fillRule="evenodd" d="M2 10a8 8 0 1116 0 8 8 0 01-16 0zm6.39-2.908a.75.75 0 01.766.027l3.5 2.25a.75.75 0 010 1.262l-3.5 2.25A.75.75 0 018 12.25v-4.5a.75.75 0 01.39-.658z" clipRule="evenodd" />
                  </svg>
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  title="Delete plan"
                  onClick={() => setDeleteTarget(plan)}
                  className="text-gray-500 hover:text-red-400"
                >
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
                    <path fillRule="evenodd" d="M8.75 1A2.75 2.75 0 006 3.75v.443c-.795.077-1.584.176-2.365.298a.75.75 0 10.23 1.482l.149-.022.841 10.518A2.75 2.75 0 007.596 19h4.807a2.75 2.75 0 002.742-2.53l.841-10.52.149.023a.75.75 0 00.23-1.482A41.03 41.03 0 0014 4.193v-.443A2.75 2.75 0 0011.25 1h-2.5zM10 4c.84 0 1.673.025 2.5.075V3.75c0-.69-.56-1.25-1.25-1.25h-2.5c-.69 0-1.25.56-1.25 1.25v.325C8.327 4.025 9.16 4 10 4zM8.58 7.72a.75.75 0 00-1.5.06l.3 7.5a.75.75 0 101.5-.06l-.3-7.5zm4.34.06a.75.75 0 10-1.5-.06l-.3 7.5a.75.75 0 101.5.06l.3-7.5z" clipRule="evenodd" />
                  </svg>
                </Button>
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
                      {progress && progress.secondsRemaining != null
                        ? `~${formatDuration(progress.secondsRemaining)} remaining`
                        : progress
                        ? `${formatDuration(progress.secondsElapsed)} elapsed`
                        : ""}
                    </span>
                  </div>
                  <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                    <div
                      className="bg-blue-500 h-2 rounded-full transition-all duration-500"
                      style={{ width: `${((progress?.percentDone ?? 0) * 100).toFixed(1)}%` }}
                    />
                  </div>
                  <p className="text-xs text-gray-500 font-mono truncate" title={progress?.currentFiles[0] ?? ""}>
                    {progress && progress.currentFiles.length > 0 ? progress.currentFiles[0] : " "}
                  </p>
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
