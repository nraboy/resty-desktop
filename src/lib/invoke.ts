import { invoke } from "@tauri-apps/api/core";
import type { BackupPlan, FileEntry, Repository, ResticStats, RetentionPolicy, Snapshot } from "./types";

export const listRepos = (): Promise<Repository[]> =>
  invoke("list_repos");

export const addRepo = (repo: Repository): Promise<void> =>
  invoke("add_repo", { repo });

export const removeRepo = (repoId: string): Promise<void> =>
  invoke("remove_repo", { repoId });

export const renameRepo = (repoId: string, newName: string): Promise<void> =>
  invoke("rename_repo", { repoId, newName });

export const initRepo = (repo: Repository): Promise<void> =>
  invoke("init_repo", { repo });

export const checkRepo = (repo: Repository): Promise<void> =>
  invoke("check_repo", { repo });

export const getRepoStats = (repo: Repository): Promise<ResticStats> =>
  invoke("get_repo_stats", { repo });

export const getResticPath = (): Promise<string> =>
  invoke("get_restic_path");

export const setResticPath = (path: string): Promise<void> =>
  invoke("set_restic_path", { path });

export const listSnapshots = (repo: Repository): Promise<Snapshot[]> =>
  invoke("list_snapshots", { repo });

export const deleteSnapshot = (
  repo: Repository,
  snapshotId: string,
  prune: boolean
): Promise<void> =>
  invoke("delete_snapshot", { repo, snapshotId, prune });

export const tagSnapshot = (
  repo: Repository,
  snapshotId: string,
  addTags: string[],
  removeTags: string[]
): Promise<void> =>
  invoke("tag_snapshot", { repo, snapshotId, addTags, removeTags });

export const runBackup = (
  repo: Repository,
  paths: string[],
  tags: string[],
  excludes: string[]
): Promise<string> =>
  invoke("run_backup", { repo, paths, tags, excludes });

export const listFiles = (
  repo: Repository,
  snapshotId: string,
  path?: string
): Promise<FileEntry[]> =>
  invoke("list_files", { repo, snapshotId, path });

export const restorePath = (
  repo: Repository,
  snapshotId: string,
  includePath: string,
  targetDir: string
): Promise<void> =>
  invoke("restore_path", { repo, snapshotId, includePath, targetDir });

export const listBackupPlans = (): Promise<BackupPlan[]> =>
  invoke("list_backup_plans");

export const saveBackupPlan = (plan: BackupPlan): Promise<void> =>
  invoke("save_backup_plan", { plan });

export const removeBackupPlan = (planId: string): Promise<void> =>
  invoke("remove_backup_plan", { planId });

export const forgetByPlan = (
  repo: Repository,
  tags: string[],
  paths: string[],
  retention: RetentionPolicy
): Promise<string> =>
  invoke("forget_by_plan", { repo, tags, paths, retention });
