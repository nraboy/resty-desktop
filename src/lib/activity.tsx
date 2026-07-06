// Powers the Activity panel (src/components/ActivityPanel.tsx). Hoisted to a top-level
// provider (mounted once in App.tsx) rather than living in a page, because — unlike every
// other progress listener in this app (BackupPlansPage, SnapshotsPage, SettingsPage) — it
// must keep updating no matter which route is currently mounted. Tauri events broadcast, so
// these listeners coexist peacefully with the page-local ones that already exist.
//
// Scope is deliberately narrow: only activity the user has no other visibility into —
// background auto-indexing and scheduler-triggered backups. Restore/copy/mirror/manual
// backup/prune already have their own progress modals and are intentionally excluded here.
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getAutoIndexing,
  getIndexProgress,
  listBackupHistory,
  listBackupPlans,
  listSchedules,
} from "./invoke";
import type { BackupHistoryEntry, BackupProgress, Schedule } from "./types";

export interface UpcomingBackup {
  scheduleId: string;
  scheduleName: string;
  planNames: string[];
  nextRunAt: number;
}

export interface ActiveScheduledBackup {
  scheduleName: string;
  planName: string;
  progress: BackupProgress | null;
}

interface ActivityState {
  /** Background auto-indexing progress; null when auto-indexing is off or fully caught up. */
  indexing: { cached: number; total: number } | null;
  /** The scheduler-triggered backup currently running, if any. Manual/"Run Now" backups
   *  never populate this — see scheduler.rs's `scheduler:backup-started`. */
  activeBackup: ActiveScheduledBackup | null;
  /** Next three enabled, due schedules, soonest first. */
  upcoming: UpcomingBackup[];
  /** Last three backup history entries, newest first. */
  recentLogs: BackupHistoryEntry[];
  /** Bumped every 60s so relative-time labels ("in 3 hours") stay fresh without a refetch. */
  clockTick: number;
}

const ActivityContext = createContext<ActivityState | null>(null);

export function useActivity(): ActivityState {
  const ctx = useContext(ActivityContext);
  if (!ctx) throw new Error("useActivity must be used within an ActivityProvider");
  return ctx;
}

async function loadUpcoming(): Promise<UpcomingBackup[]> {
  const [schedules, plans] = await Promise.all([listSchedules(), listBackupPlans()]);
  const planNameOf = (id: string) => plans.find((p) => p.id === id)?.name ?? id;

  return schedules
    .filter((s): s is Schedule & { nextRunAt: number } => s.enabled && s.nextRunAt != null)
    .sort((a, b) => a.nextRunAt - b.nextRunAt)
    .slice(0, 3)
    .map((s) => ({
      scheduleId: s.id,
      scheduleName: s.name,
      planNames: s.planIds.map(planNameOf),
      nextRunAt: s.nextRunAt,
    }));
}

async function loadIndexing(): Promise<{ cached: number; total: number } | null> {
  const enabled = await getAutoIndexing();
  if (!enabled) return null;
  const progress = await getIndexProgress();
  return progress.total > progress.cached ? progress : null;
}

export function ActivityProvider({ children }: { children: ReactNode }) {
  const [indexing, setIndexing] = useState<{ cached: number; total: number } | null>(null);
  const [activeBackup, setActiveBackup] = useState<ActiveScheduledBackup | null>(null);
  const [upcoming, setUpcoming] = useState<UpcomingBackup[]>([]);
  const [recentLogs, setRecentLogs] = useState<BackupHistoryEntry[]>([]);
  const [clockTick, setClockTick] = useState(0);
  // Holds name/plan between "started" and the first "backup:progress" payload.
  const pendingBackupRef = useRef<{ scheduleName: string; planName: string } | null>(null);

  const refreshIndexing = () => { loadIndexing().then(setIndexing).catch(() => {}); };
  const refreshUpcoming = () => { loadUpcoming().then(setUpcoming).catch(() => {}); };
  const refreshLogs = () => { listBackupHistory().then((h) => setRecentLogs(h.slice(0, 3))).catch(() => {}); };

  useEffect(() => {
    refreshIndexing();
    refreshUpcoming();
    refreshLogs();

    const tickTimer = setInterval(() => setClockTick((t) => t + 1), 60_000);

    const unlistenIndexDone = listen("index:done", refreshIndexing);
    const unlistenSnapshotsRefreshed = listen("snapshots:refreshed", refreshIndexing);

    const unlistenBackupStarted = listen<{ scheduleName: string; planName: string }>(
      "scheduler:backup-started",
      (e) => {
        pendingBackupRef.current = e.payload;
        setActiveBackup({ ...e.payload, progress: null });
      }
    );
    const unlistenBackupProgress = listen<BackupProgress>("backup:progress", (e) => {
      if (!pendingBackupRef.current) return; // not a scheduler-triggered backup — ignore
      setActiveBackup({ ...pendingBackupRef.current, progress: e.payload });
    });
    const unlistenBackupFinished = listen("scheduler:backup-finished", () => {
      pendingBackupRef.current = null;
      setActiveBackup(null);
      refreshUpcoming();
    });
    // Fires for every backup, manual or scheduled (snapshot.rs's execute_backup) — unlike
    // scheduler:backup-finished above, which only covers scheduler-triggered runs and would
    // otherwise leave a manually-run backup missing from Recent Logs until the next
    // scheduled run happened to refresh it.
    const unlistenHistoryUpdated = listen("backup:history-updated", refreshLogs);

    return () => {
      clearInterval(tickTimer);
      unlistenIndexDone.then((fn) => fn());
      unlistenSnapshotsRefreshed.then((fn) => fn());
      unlistenBackupStarted.then((fn) => fn());
      unlistenBackupProgress.then((fn) => fn());
      unlistenBackupFinished.then((fn) => fn());
      unlistenHistoryUpdated.then((fn) => fn());
    };
  }, []);

  return (
    <ActivityContext.Provider value={{ indexing, activeBackup, upcoming, recentLogs, clockTick }}>
      {children}
    </ActivityContext.Provider>
  );
}
