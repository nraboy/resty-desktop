import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { listSchedules, removeSchedule, toggleSchedule } from "../lib/invoke";
import type { Schedule } from "../lib/types";
import { formatTimestamp } from "../lib/format";
import Button from "../components/Button";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

export default function SchedulesPage() {
  const navigate = useNavigate();
  const [schedules, setSchedules] = useState<Schedule[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<Schedule | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [toggling, setToggling] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      setSchedules(await listSchedules());
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);

  const handleToggle = async (sched: Schedule) => {
    setToggling(sched.id);
    try {
      await toggleSchedule(sched.id, !sched.enabled);
      await load();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setToggling(null);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await removeSchedule(deleteTarget.id);
      setDeleteTarget(null);
      await load();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setDeleting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-gray-500 text-sm">Loading schedules…</p>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Schedules</h1>
          <p className="text-sm text-gray-500 mt-0.5">
            Automate backup plans on a recurring schedule.
          </p>
        </div>
        <Button onClick={() => navigate("/schedules/new")}>New Schedule</Button>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {schedules.length === 0 ? (
        <EmptyState
          title="No schedules"
          description="Create a schedule to automatically run backup plans at set times."
          action={<Button onClick={() => navigate("/schedules/new")}>Create a Schedule</Button>}
        />
      ) : (
        <div className="space-y-3">
          {schedules.map((sched) => (
            <div
              key={sched.id}
              className="bg-gray-900 border border-gray-800 rounded-xl px-5 py-4 flex items-center gap-4"
            >
              <div
                className="flex-1 min-w-0 cursor-pointer"
                onClick={() => navigate(`/schedules/${sched.id}`)}
              >
                <p className="text-sm font-medium text-gray-100 truncate">{sched.name}</p>
                <p className="text-xs text-gray-500 mt-0.5">
                  {sched.cronExpr}
                  {" · "}
                  {sched.planIds.length} {sched.planIds.length === 1 ? "plan" : "plans"}
                </p>
                <div className="flex gap-4 mt-1 text-xs text-gray-600">
                  <span>Last run: {formatTimestamp(sched.lastRunAt)}</span>
                  <span>Next run: {sched.enabled ? formatTimestamp(sched.nextRunAt) : "—"}</span>
                </div>
              </div>

              <div className="flex items-center gap-1 flex-shrink-0">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setDeleteTarget(sched)}
                  className="text-gray-500 hover:text-red-400"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                  </svg>
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => navigate(`/schedules/${sched.id}`)}
                  className="text-gray-500 hover:text-blue-400"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                  </svg>
                </Button>
                <button
                  onClick={() => handleToggle(sched)}
                  disabled={toggling === sched.id}
                  className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none disabled:opacity-50 ml-2 ${
                    sched.enabled ? "bg-blue-600" : "bg-gray-700"
                  }`}
                >
                  <span
                    className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
                      sched.enabled ? "translate-x-4" : "translate-x-1"
                    }`}
                  />
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        title="Delete Schedule"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-gray-50">{deleteTarget?.name}</span>?
          This removes the schedule only — backup plans are not affected.
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
    </div>
  );
}
