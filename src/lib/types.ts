export interface Repository {
  id: string;
  name: string;
  path: string;
}

const REMOTE_PREFIXES = ["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];
export function isRemoteRepo(path: string): boolean {
  return REMOTE_PREFIXES.some((p) => path.startsWith(p));
}

export interface Snapshot {
  id: string;
  short_id: string;
  time: string;
  hostname: string;
  username?: string;
  paths: string[];
  tags?: string[];
}

export interface FileEntry {
  name: string;
  path: string;
  type: "file" | "dir" | "symlink" | "other";
  size?: number;
  mtime?: string;
  mode?: number;
}

export interface ResticStats {
  total_size: number;
  total_file_count: number;
  snapshots_count: number;
}

export interface BackupHistoryEntry {
  id: string;
  repoId: string;
  repoName?: string;
  planId?: string;
  planName?: string;
  snapshotId?: string;
  startedAt: number;
  durationSeconds: number;
  filesNew: number;
  filesChanged: number;
  bytesAdded: number;
  error?: string;
}

export interface BackupProgress {
  percentDone: number;
  filesDone: number;
  totalFiles: number;
  bytesDone: number;
  totalBytes: number;
  secondsElapsed: number;
  secondsRemaining?: number;
  currentFiles: string[];
}

export interface RestoreProgress {
  percentDone: number;
  filesRestored: number;
  totalFiles: number;
  bytesRestored: number;
  totalBytes: number;
  secondsElapsed: number;
}

export interface CheckResult {
  success: boolean;
  errors: string[];
  duration_seconds: number;
}

export interface RetentionPolicy {
  keepLast?: number;
  keepDaily?: number;
  keepWeekly?: number;
  keepMonthly?: number;
  keepYearly?: number;
}

export interface BackupPlan {
  id: string;
  name: string;
  repoId: string;
  paths: string[];
  tags: string[];
  excludes: string[];
  retention?: RetentionPolicy;
}

export interface Schedule {
  id: string;
  name: string;
  planIds: string[];
  cronExpr: string;
  enabled: boolean;
  lastRunAt?: number;
  nextRunAt?: number;
  createdAt: number;
}

export type ScheduleFrequency = "hourly" | "daily" | "weekly" | "monthly" | "custom";
