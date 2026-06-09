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
