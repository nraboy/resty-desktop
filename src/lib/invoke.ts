import { invoke } from "@tauri-apps/api/core";
import type { BackupHistoryEntry, BackupPlan, CheckResult, DiffResult, FileEntry, Repository, ResticStats, RetentionPolicy, Schedule, Snapshot, SnapshotStats } from "./types";

// ── auth ──────────────────────────────────────────────────────────────────

export const isAppSetup = (): Promise<boolean> =>
  invoke("is_app_setup");

export const setMenuAuthState = (unlocked: boolean): Promise<void> =>
  invoke("set_menu_auth_state", { unlocked });

export const activateTray = (): Promise<void> =>
  invoke("activate_tray");

export const deactivateTray = (): Promise<void> =>
  invoke("deactivate_tray");

export const setupMasterPassword = (password: string): Promise<void> =>
  invoke("setup_master_password", { password });

export const unlockApp = (password: string): Promise<void> =>
  invoke("unlock_app", { password });

export const lockApp = (): Promise<void> =>
  invoke("lock_app");

export const changeMasterPassword = (oldPassword: string, newPassword: string): Promise<void> =>
  invoke("change_master_password", { oldPassword, newPassword });

export const resetApp = (): Promise<void> =>
  invoke("reset_app");

// ── repos ─────────────────────────────────────────────────────────────────

export const listRepos = (): Promise<Repository[]> =>
  invoke("list_repos");

export const addRepo = (id: string, name: string, path: string, password: string): Promise<void> =>
  invoke("add_repo", { id, name, path, password });

export const removeRepo = (repoId: string): Promise<void> =>
  invoke("remove_repo", { repoId });

export const renameRepo = (repoId: string, newName: string): Promise<void> =>
  invoke("rename_repo", { repoId, newName });

export const updateRepoPath = (repoId: string, newPath: string): Promise<void> =>
  invoke("update_repo_path", { repoId, newPath });

export const getRepoPassword = (repoId: string): Promise<string> =>
  invoke("get_repo_password", { repoId });

export const updateRepoPassword = (repoId: string, newPassword: string): Promise<void> =>
  invoke("update_repo_password", { repoId, newPassword });

export const initRepo = (id: string, name: string, path: string, password: string): Promise<void> =>
  invoke("init_repo", { id, name, path, password });

export const testRepoConnection = (path: string, password: string): Promise<void> =>
  invoke("test_repo_connection", { path, password });

export const getRepoStats = (repoId: string): Promise<ResticStats> =>
  invoke("get_repo_stats", { repoId });

export const refreshRepoStats = (repoId: string): Promise<ResticStats> =>
  invoke("refresh_repo_stats", { repoId });

export const getResticPath = (): Promise<string> =>
  invoke("get_restic_path");

export const setResticPath = (path: string): Promise<void> =>
  invoke("set_restic_path", { path });

export const getResticVersion = (): Promise<string> =>
  invoke("get_restic_version");

export const getCompression = (): Promise<string> =>
  invoke("get_compression");

export const setCompression = (value: string): Promise<void> =>
  invoke("set_compression", { value });

export const getRestorePath = (): Promise<string> =>
  invoke("get_restore_path");

export const setRestorePath = (path: string): Promise<void> =>
  invoke("set_restore_path", { path });

export const getTrayEnabled = (): Promise<boolean> =>
  invoke("get_tray_enabled");

export const setTrayEnabled = (value: boolean): Promise<void> =>
  invoke("set_tray_enabled", { value });

export const checkRepo = (repoId: string): Promise<CheckResult> =>
  invoke("check_repo", { repoId });

export const pruneAllRepos = (): Promise<void> =>
  invoke("prune_all_repos");

export const pruneRepo = (repoId: string): Promise<void> =>
  invoke("prune_repo", { repoId });

export const cancelPrune = (): Promise<void> =>
  invoke("cancel_prune");

// ── snapshots ─────────────────────────────────────────────────────────────

export const listSnapshots = (repoId: string): Promise<Snapshot[]> =>
  invoke("list_snapshots", { repoId });

export const getSnapshotStats = (repoId: string, snapshotId: string): Promise<SnapshotStats> =>
  invoke("get_snapshot_stats", { repoId, snapshotId });

export const refreshSnapshots = (repoId: string): Promise<Snapshot[]> =>
  invoke("refresh_snapshots", { repoId });

export const deleteSnapshot = (repoId: string, snapshotId: string, prune: boolean): Promise<void> =>
  invoke("delete_snapshot", { repoId, snapshotId, prune });

export const tagSnapshot = (
  repoId: string,
  snapshotId: string,
  addTags: string[],
  removeTags: string[]
): Promise<void> =>
  invoke("tag_snapshot", { repoId, snapshotId, addTags, removeTags });

export const runBackup = (
  repoId: string,
  paths: string[],
  tags: string[],
  excludes: string[],
  planId?: string,
  limitUpload?: number,
  limitDownload?: number,
): Promise<string> =>
  invoke("run_backup", { repoId, paths, tags, excludes, planId: planId ?? null, limitUpload: limitUpload ?? null, limitDownload: limitDownload ?? null });

export const unlockRepo = (repoId: string): Promise<void> =>
  invoke("unlock_repo", { repoId });

export const copySnapshot = (
  srcRepoId: string,
  destRepoId: string,
  snapshotId: string
): Promise<void> =>
  invoke("copy_snapshot", { srcRepoId, destRepoId, snapshotId });

export const cancelCopy = (): Promise<void> =>
  invoke("cancel_copy");

export const mirrorRepo = (srcRepoId: string, destRepoId: string): Promise<void> =>
  invoke("mirror_repo", { srcRepoId, destRepoId });

export const cancelMirror = (): Promise<void> =>
  invoke("cancel_mirror");

export const cancelBackup = (): Promise<void> =>
  invoke("cancel_backup");

export const diffSnapshots = (repoId: string, snapshotA: string, snapshotB: string): Promise<DiffResult> =>
  invoke("diff_snapshots", { repoId, snapshotA, snapshotB });

export const forgetByPlan = (
  repoId: string,
  tags: string[],
  paths: string[],
  retention: RetentionPolicy
): Promise<string> =>
  invoke("forget_by_plan", { repoId, tags, paths, retention });

// ── browse ────────────────────────────────────────────────────────────────

export const listFiles = (repoId: string, snapshotId: string, path?: string): Promise<FileEntry[]> =>
  invoke("list_files", { repoId, snapshotId, path });

export const restorePath = (
  repoId: string,
  snapshotId: string,
  includePath: string,
  targetDir: string
): Promise<void> =>
  invoke("restore_path", { repoId, snapshotId, includePath, targetDir });

export const restoreSnapshot = (
  repoId: string,
  snapshotId: string,
  targetDir: string
): Promise<void> =>
  invoke("restore_snapshot", { repoId, snapshotId, targetDir });

// ── backup plans ──────────────────────────────────────────────────────────

export const listBackupPlans = (): Promise<BackupPlan[]> =>
  invoke("list_backup_plans");

export const saveBackupPlan = (plan: BackupPlan): Promise<void> =>
  invoke("save_backup_plan", { plan });

export const removeBackupPlan = (planId: string): Promise<void> =>
  invoke("remove_backup_plan", { planId });

// ── schedules ─────────────────────────────────────────────────────────────

export const listSchedules = (): Promise<Schedule[]> =>
  invoke("list_schedules");

export const saveSchedule = (schedule: Schedule): Promise<void> =>
  invoke("save_schedule", { schedule });

export const removeSchedule = (scheduleId: string): Promise<void> =>
  invoke("remove_schedule", { scheduleId });

export const toggleSchedule = (scheduleId: string, enabled: boolean): Promise<void> =>
  invoke("toggle_schedule", { scheduleId, enabled });

export const runScheduleNow = (scheduleId: string): Promise<void> =>
  invoke("run_schedule_now", { scheduleId });

export const describeCronExpr = (cronExpr: string): Promise<string> =>
  invoke("describe_cron_expr", { cronExpr });

// ── cache ─────────────────────────────────────────────────────────────────

export const clearBrowseCache = (): Promise<void> =>
  invoke("clear_browse_cache");

export const listBackupHistory = (): Promise<BackupHistoryEntry[]> =>
  invoke("list_backup_history");
