import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listBackupPlans, removeBackupPlan, listRepos, runBackup, forgetByPlan } from "../lib/invoke";
import type { BackupPlan, Repository } from "../lib/types";
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
  const [backupOutput, setBackupOutput] = useState("");
  const [backupError, setBackupError] = useState("");
  const [backupDone, setBackupDone] = useState(false);

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
    setBackupOutput("");
    setBackupError("");
    setBackupDone(false);
  };

  const closeBackupModal = () => {
    if (backupRunning) return;
    setBackupPlan(null);
  };

  const startBackup = async () => {
    if (!backupPlan) return;
    const repo = repos.find((r) => r.id === backupPlan.repoId);
    if (!repo) {
      setBackupError("Repository not found. Check the plan configuration.");
      return;
    }
    setBackupRunning(true);
    setBackupError("");
    setBackupOutput("");
    setBackupDone(false);
    try {
      const result = await runBackup(repo, backupPlan.paths, backupPlan.tags, backupPlan.excludes);
      let output = result;
      if (backupPlan.retention) {
        try {
          const pruneResult = await forgetByPlan(repo, backupPlan.tags, backupPlan.paths, backupPlan.retention);
          output += "\n\n--- Pruning ---\n" + pruneResult;
        } catch (pruneErr: any) {
          output += "\n\n--- Pruning failed ---\n" + String(pruneErr);
        }
      }
      setBackupOutput(output);
      setBackupDone(true);
    } catch (err: any) {
      setBackupError(String(err));
    } finally {
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
        <Button onClick={() => navigate("/backup-plans/new")}>
          New Plan
        </Button>
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
          action={
            <Button onClick={() => navigate("/backup-plans/new")}>Create a Plan</Button>
          }
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
                  variant="secondary"
                  size="sm"
                  onClick={() => navigate(`/backup-plans/${plan.id}`)}
                >
                  Edit
                </Button>
                <Button size="sm" onClick={() => openBackupModal(plan)}>
                  Backup Now
                </Button>
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

      {/* Delete confirmation modal */}
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
          <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={deleting}>
            Cancel
          </Button>
          <Button variant="danger" loading={deleting} onClick={handleDelete}>
            Delete
          </Button>
        </div>
      </Modal>

      {/* Backup now modal */}
      <Modal
        title={backupPlan ? `Backup: ${backupPlan.name}` : "Backup"}
        open={!!backupPlan}
        onClose={closeBackupModal}
      >
        {backupPlan && (
          <div className="space-y-3">
            <div className="text-sm text-gray-400 space-y-1">
              <p>
                <span className="text-gray-500">Repository:</span>{" "}
                <span className="text-gray-200">{repoName(backupPlan.repoId)}</span>
              </p>
              <p>
                <span className="text-gray-500">Paths:</span>{" "}
                <span className="text-gray-200 break-all">
                  {backupPlan.paths.join(", ") || "None"}
                </span>
              </p>
            </div>

            {backupError && (
              <div className="p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 font-mono whitespace-pre-wrap">
                {backupError}
              </div>
            )}

            {backupOutput && (
              <div className="p-3 bg-gray-800 border border-gray-700 rounded-lg text-xs text-gray-300 font-mono whitespace-pre-wrap max-h-48 overflow-y-auto">
                {backupOutput}
              </div>
            )}

            {backupDone && (
              <div className="p-3 bg-green-900/30 border border-green-700 rounded-lg text-sm text-green-300">
                Backup completed successfully.
              </div>
            )}

            <div className="flex justify-end gap-2 pt-1">
              {backupDone ? (
                <Button variant="secondary" onClick={closeBackupModal}>Close</Button>
              ) : (
                <>
                  <Button
                    variant="secondary"
                    onClick={closeBackupModal}
                    disabled={backupRunning}
                  >
                    Cancel
                  </Button>
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
