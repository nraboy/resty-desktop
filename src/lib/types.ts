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

// A file match from a repo-wide search, attributed to the (newest) snapshot
// containing it so the frontend can open the correct BrowsePage.
export interface RepoFileHit {
  name: string;
  path: string;
  type: "file" | "dir" | "symlink" | "other";
  size?: number;
  mtime?: string;
  mode?: number;
  snapshotId: string;
  snapshotShortId: string;
}

export interface ResticStats {
  total_size: number;
  total_file_count: number;
  snapshots_count: number;
}

export interface SnapshotStats {
  totalSize: number;
  totalFileCount: number;
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

export interface IndexProgress {
  cached: number;
  total: number;
}

export interface CheckResult {
  success: boolean;
  errors: string[];
  duration_seconds: number;
}

export interface DiffEntry {
  path: string;
  change: "added" | "removed" | "modified";
}

export interface DiffResult {
  entries: DiffEntry[];
  totalAdded: number;
  totalRemoved: number;
  totalModified: number;
  truncated: boolean;
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
  limitUpload?: number;
  limitDownload?: number;
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

export interface ExportSummary {
  repos: number;
  plans: number;
  schedules: number;
}

export interface ImportPreview {
  repos: number;
  plans: number;
  schedules: number;
  requiresPassword: boolean;
}
