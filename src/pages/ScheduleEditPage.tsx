import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { describeCronExpr, listBackupPlans, listRepos, listSchedules, removeSchedule, saveSchedule } from "../lib/invoke";
import { parseCronToSimple, buildCronExpr } from "../lib/cron";
import type { BackupPlan, Repository, Schedule, ScheduleFrequency } from "../lib/types";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import { ChevronDownIcon } from "../components/icons";

const DAYS_OF_WEEK = [
  { value: "0", label: "Sunday" },
  { value: "1", label: "Monday" },
  { value: "2", label: "Tuesday" },
  { value: "3", label: "Wednesday" },
  { value: "4", label: "Thursday" },
  { value: "5", label: "Friday" },
  { value: "6", label: "Saturday" },
];

type CronMode = "simple" | "expert";

export default function ScheduleEditPage() {
  const { scheduleId } = useParams<{ scheduleId: string }>();
  const navigate = useNavigate();
  const isNew = scheduleId === "new";

  const [plans, setPlans] = useState<BackupPlan[]>([]);
  // Loaded solely to badge a plan's repo as read-only in the picker below — a schedule
  // has no repoId of its own (see SchedulesPage.tsx's identical scheduleHasReadOnlyRepo).
  const [repos, setRepos] = useState<Repository[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState("");
  const [showDelete, setShowDelete] = useState(false);

  const [name, setName] = useState("");
  const [selectedPlanIds, setSelectedPlanIds] = useState<string[]>([]);
  const [enabled, setEnabled] = useState(true);
  const [cronMode, setCronMode] = useState<CronMode>("simple");

  // Simple mode fields
  const [frequency, setFrequency] = useState<ScheduleFrequency>("daily");
  const [hour, setHour] = useState("2");
  const [minute, setMinute] = useState("0");
  const [dayOfWeek, setDayOfWeek] = useState("1");
  const [dayOfMonth, setDayOfMonth] = useState("1");

  // Expert mode
  const [cronExpr, setCronExpr] = useState("");
  const [cronDescription, setCronDescription] = useState("");

  useEffect(() => {
    const init = async () => {
      try {
        const [allPlans, allSchedules, allRepos] = await Promise.all([
          listBackupPlans(),
          listSchedules(),
          listRepos(),
        ]);
        setPlans(allPlans);
        setRepos(allRepos);
        if (!isNew) {
          const existing = allSchedules.find((s) => s.id === scheduleId);
          if (existing) {
            setName(existing.name);
            setSelectedPlanIds(existing.planIds);
            setEnabled(existing.enabled);
            const simple = parseCronToSimple(existing.cronExpr);
            if (simple) {
              setFrequency(simple.frequency);
              setHour(simple.hour);
              setMinute(simple.minute);
              setDayOfWeek(simple.dayOfWeek);
              setDayOfMonth(simple.dayOfMonth);
              setCronMode("simple");
            } else {
              setCronExpr(existing.cronExpr);
              setCronMode("expert");
            }
          }
        }
      } catch (err: any) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    };
    init();
  }, [scheduleId, isNew]);

  // Refresh cron description when expert expression changes
  useEffect(() => {
    if (cronMode !== "expert" || !cronExpr.trim()) {
      setCronDescription("");
      return;
    }
    describeCronExpr(cronExpr)
      .then(setCronDescription)
      .catch(() => setCronDescription(""));
  }, [cronExpr, cronMode]);

  const effectiveCronExpr =
    cronMode === "simple"
      ? buildCronExpr(frequency, hour, minute, dayOfWeek, dayOfMonth)
      : cronExpr;

  const togglePlan = (planId: string) => {
    setSelectedPlanIds((prev) =>
      prev.includes(planId) ? prev.filter((id) => id !== planId) : [...prev, planId]
    );
  };

  // Currently-selected plans whose repo is read-only — these are the ones that will
  // actually fail if this schedule is saved as-is (see execute_backup's ensure_writable
  // guard, snapshot.rs), not just any read-only-badged plan in the full list above.
  const selectedReadOnlyPlans = plans.filter(
    (plan) => selectedPlanIds.includes(plan.id) && (repos.find((r) => r.id === plan.repoId)?.readOnly ?? false)
  );

  const handleSave = async () => {
    setError("");
    if (!name.trim()) { setError("Schedule name is required."); return; }
    if (selectedPlanIds.length === 0) { setError("Select at least one backup plan."); return; }
    if (!effectiveCronExpr.trim()) { setError("A schedule expression is required."); return; }

    setSaving(true);
    try {
      const schedule: Schedule = {
        id: isNew ? crypto.randomUUID() : scheduleId!,
        name: name.trim(),
        planIds: selectedPlanIds,
        cronExpr: effectiveCronExpr.trim(),
        enabled,
        lastRunAt: undefined,
        nextRunAt: undefined,
        createdAt: Math.floor(Date.now() / 1000),
      };
      await saveSchedule(schedule);
      navigate("/schedules");
    } catch (err: any) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    setDeleting(true);
    try {
      await removeSchedule(scheduleId!);
      navigate("/schedules");
    } catch (err: any) {
      setError(String(err));
      setDeleting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-gray-500 text-sm">Loading…</p>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="mb-6">
        <h1 className="text-xl font-semibold text-gray-100">
          {isNew ? "New Schedule" : "Edit Schedule"}
        </h1>
        <p className="text-sm text-gray-500 mt-0.5">
          Define when to automatically run backup plans.
        </p>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      <div className="space-y-6">
        {/* Name */}
        <div>
          <Input
            label="Schedule Name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Nightly Backups"
          />
        </div>

        {/* Backup plans */}
        <div>
          <label className="text-sm font-medium text-gray-300 block mb-2">Backup Plans</label>
          {plans.length === 0 ? (
            <p className="text-sm text-gray-500 bg-gray-900 border border-gray-800 rounded-xl p-4">
              No backup plans exist yet. Create one first.
            </p>
          ) : (
            <div className="bg-gray-900 border border-gray-800 rounded-xl divide-y divide-gray-800">
              {plans.map((plan) => (
                <label
                  key={plan.id}
                  className="flex items-center gap-3 px-4 py-3 cursor-pointer hover:bg-gray-800/50 transition-colors"
                >
                  <input
                    type="checkbox"
                    checked={selectedPlanIds.includes(plan.id)}
                    onChange={() => togglePlan(plan.id)}
                    className="rounded border-gray-600 bg-gray-800 text-blue-400 focus:ring-blue-500 focus:ring-offset-gray-900"
                  />
                  <div>
                    <div className="flex items-center gap-2">
                      <p className="text-sm text-gray-200">{plan.name}</p>
                      {repos.find((r) => r.id === plan.repoId)?.readOnly && (
                        <span
                          className="text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded bg-gray-800 border border-gray-700 text-gray-400"
                          title="This plan's repository is read-only — scheduled backups against it will fail."
                        >
                          Read-only repo
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-gray-500">
                      {plan.paths.length} {plan.paths.length === 1 ? "path" : "paths"}
                    </p>
                  </div>
                </label>
              ))}
            </div>
          )}
          {selectedReadOnlyPlans.length > 0 && (
            <p className="text-sm text-amber-400 mt-2">
              One or more of the selected plans target a read-only repository — every time this
              schedule runs, those plans' backups will fail (the rest of the schedule still runs
              normally), until you either pick a different repository for them or clear their
              read-only flag on Repositories.
            </p>
          )}
        </div>

        {/* Schedule timing */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <label className="text-sm font-medium text-gray-300">Schedule</label>
            <div className="flex rounded-lg overflow-hidden border border-gray-700 text-xs">
              {(["simple", "expert"] as CronMode[]).map((mode) => (
                <button
                  key={mode}
                  onClick={() => {
                    if (mode === "expert" && cronMode === "simple") {
                      setCronExpr(effectiveCronExpr);
                    } else if (mode === "simple" && cronMode === "expert") {
                      const simple = parseCronToSimple(cronExpr);
                      if (simple) {
                        setFrequency(simple.frequency);
                        setHour(simple.hour);
                        setMinute(simple.minute);
                        setDayOfWeek(simple.dayOfWeek);
                        setDayOfMonth(simple.dayOfMonth);
                      }
                    }
                    setCronMode(mode);
                  }}
                  className={`px-3 py-1 capitalize transition-colors ${
                    cronMode === mode
                      ? "bg-blue-600 text-white"
                      : "bg-gray-800 text-gray-400 hover:text-gray-200"
                  }`}
                >
                  {mode}
                </button>
              ))}
            </div>
          </div>

          {cronMode === "simple" ? (
            <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 space-y-4">
              {/* Frequency */}
              <div>
                <label className="text-xs text-gray-500 mb-1.5 block">Frequency</label>
                <div className="flex gap-2">
                  {(["hourly", "daily", "weekly", "monthly"] as ScheduleFrequency[]).map((f) => (
                    <button
                      key={f}
                      onClick={() => setFrequency(f)}
                      className={`px-3 py-1.5 rounded-lg text-xs capitalize transition-colors border ${
                        frequency === f
                          ? "bg-blue-600 text-white border-blue-500"
                          : "bg-gray-800 text-gray-400 border-gray-700 hover:text-gray-200"
                      }`}
                    >
                      {f}
                    </button>
                  ))}
                </div>
              </div>

              {/* Day of week (weekly only) */}
              {frequency === "weekly" && (
                <div>
                  <label className="text-xs text-gray-500 mb-1.5 block">Day of Week</label>
                  <div className="relative inline-block">
                    <select
                      value={dayOfWeek}
                      onChange={(e) => setDayOfWeek(e.target.value)}
                      className="appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                    >
                      {DAYS_OF_WEEK.map((d) => (
                        <option key={d.value} value={d.value}>{d.label}</option>
                      ))}
                    </select>
                    <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
                  </div>
                </div>
              )}

              {/* Day of month (monthly only) */}
              {frequency === "monthly" && (
                <div>
                  <label className="text-xs text-gray-500 mb-1.5 block">Day of Month</label>
                  <div className="relative inline-block">
                    <select
                      value={dayOfMonth}
                      onChange={(e) => setDayOfMonth(e.target.value)}
                      className="appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                    >
                      {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
                        <option key={d} value={String(d)}>{d}</option>
                      ))}
                    </select>
                    <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
                  </div>
                </div>
              )}

              {/* Time of day (hidden for hourly — minute is shown separately) */}
              {frequency !== "hourly" && (
                <div>
                  <label className="text-xs text-gray-500 mb-1.5 block">Time of Day</label>
                  <div className="flex items-center gap-2">
                    <div className="relative inline-block">
                      <select
                        value={hour}
                        onChange={(e) => setHour(e.target.value)}
                        className="appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                      >
                        {Array.from({ length: 24 }, (_, i) => i).map((h) => (
                          <option key={h} value={String(h)}>{String(h).padStart(2, "0")}</option>
                        ))}
                      </select>
                      <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
                    </div>
                    <span className="text-gray-500">:</span>
                    <div className="relative inline-block">
                      <select
                        value={minute}
                        onChange={(e) => setMinute(e.target.value)}
                        className="appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                      >
                        {[0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55].map((m) => (
                          <option key={m} value={String(m)}>{String(m).padStart(2, "0")}</option>
                        ))}
                      </select>
                      <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
                    </div>
                  </div>
                </div>
              )}

              {/* Minutes past the hour (hourly only) */}
              {frequency === "hourly" && (
                <div>
                  <label className="text-xs text-gray-500 mb-1.5 block">Minutes past the hour</label>
                  <div className="relative inline-block">
                    <select
                      value={minute}
                      onChange={(e) => setMinute(e.target.value)}
                      className="appearance-none bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 pr-8 text-sm text-gray-100 focus:outline-none focus:ring-1 focus:ring-blue-500"
                    >
                      {[0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55].map((m) => (
                        <option key={m} value={String(m)}>:{String(m).padStart(2, "0")}</option>
                      ))}
                    </select>
                    <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
                  </div>
                </div>
              )}

              {effectiveCronExpr && (
                <p className="text-xs text-gray-500 font-mono">
                  cron: <span className="text-gray-400">{effectiveCronExpr}</span>
                </p>
              )}
            </div>
          ) : (
            <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 space-y-3">
              <div>
                <input
                  type="text"
                  value={cronExpr}
                  onChange={(e) => setCronExpr(e.target.value)}
                  placeholder="0 2 * * *"
                  className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-sm font-mono text-gray-200 placeholder-gray-500 focus:outline-none focus:ring-1 focus:ring-blue-500"
                />
                <p className="text-xs text-gray-500 mt-1.5">
                  5-field format: <span className="font-mono text-gray-500">minute hour day-of-month month day-of-week</span>
                </p>
              </div>
              {cronDescription && (
                <p className="text-xs text-blue-400">{cronDescription}</p>
              )}
            </div>
          )}
        </div>

        {/* Enabled toggle */}
        <div className="flex items-center justify-between bg-gray-900 border border-gray-800 rounded-xl px-4 py-3">
          <div>
            <p className="text-sm font-medium text-gray-300">Enabled</p>
            <p className="text-xs text-gray-500">Schedule will run automatically when active.</p>
          </div>
          <button
            onClick={() => setEnabled((v) => !v)}
            className={`relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none ${
              enabled ? "bg-blue-600" : "bg-gray-700"
            }`}
          >
            <span
              className={`inline-block h-3.5 w-3.5 transform rounded-full bg-white shadow transition-transform ${
                enabled ? "translate-x-4" : "translate-x-1"
              }`}
            />
          </button>
        </div>

        {/* Actions */}
        <div className="flex items-center justify-between pt-2">
          <div className="flex gap-2">
            <Button loading={saving} onClick={handleSave}>
              {isNew ? "Create Schedule" : "Save Changes"}
            </Button>
            <Button variant="secondary" onClick={() => navigate("/schedules")} disabled={saving}>
              Cancel
            </Button>
          </div>
          {!isNew && (
            <Button variant="danger" onClick={() => setShowDelete(true)}>
              Delete Schedule
            </Button>
          )}
        </div>
      </div>

      <Modal
        title="Delete Schedule"
        open={showDelete}
        onClose={() => !deleting && setShowDelete(false)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-gray-50">{name}</span>?
          This removes the schedule only — backup plans are not affected.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setShowDelete(false)} disabled={deleting}>
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
