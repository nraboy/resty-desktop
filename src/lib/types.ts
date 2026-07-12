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
  /** Unix-seconds timestamp of when this value was cached. Stats are manual-refresh-only
   *  (no background eviction) — this powers the "Refreshed …" label on RepositoriesPage. */
  cached_at?: number | null;
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

/** Sentinel `error` value for a genuinely cancelled backup — see snapshot.rs's
 *  execute_backup Err branch, the only writer of this field. Distinguishes a
 *  cancellation from a real failure so Recent Logs / LogsPage can render it
 *  neutrally rather than as an error. */
export const CANCELLED_BACKUP_ERROR = "Cancelled";

export interface BackupProgress {
  repoId: string;
  planId?: string;
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
  repoId: string;
  snapshotId: string;
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

/** Returned by `get_active_index_batch` — whether a repo already has an "Index All" batch
 *  queued or running, so a page that just (re)mounted can restore state it may have missed
 *  live `task` events for. See browse.rs's ActiveIndexBatchStatus. */
export interface ActiveIndexBatchStatus {
  operationId: string;
  started: boolean;
  /** The exact snapshot ids this batch covers — lets a page that just (re)mounted
   *  cross-reference against its own already-fetched index-status map to restore accurate
   *  local progress instead of only knowing "a batch exists." See browse.rs's
   *  ActiveIndexBatchStatus/BatchCancel::target_ids. */
  targetIds: string[];
}

/** Sentinel error returned by `index_snapshots_batch` when the repo already has a batch
 *  queued or running — matches browse.rs's `INDEX_BATCH_ALREADY_ACTIVE_ERROR` exactly, same
 *  pattern as `CANCELLED_BACKUP_ERROR` below. Lets a caller resync (re-adopt the real batch's
 *  state) instead of treating this as a genuine failure. */
export const INDEX_BATCH_ALREADY_ACTIVE_ERROR = "IndexBatchAlreadyActive";

// Unified operation lifecycle event bus (Tauri event name "task") — see
// tasks.rs and CLAUDE.md's "Operation Event Bus" section. Emitted by every
// covered restic operation alongside — not instead of — its existing detailed
// feed (BackupProgress, RestoreProgress, etc). Several frontend consumers now
// subscribe to this (see activity.tsx: stats refreshes, index lifecycle/batch
// progress, and the scheduler-triggered backup row) via a uniform,
// operationId-correlatable contract.
export type TaskKind =
  | "backup" | "restore" | "restorePath" | "copy" | "mirror" | "prune"
  | "forget" | "tag" | "check" | "diff" | "index" | "unlock" | "stats"
  | "testConnection" | "browse" | "init";
export type TaskPhase =
  | "pending" | "started" | "progress" | "cancelling" | "cancelled" | "finished" | "failed";
// "pending" (queued, not yet running) is currently only emitted by "Index All"
// batches waiting their turn on the backend's batch_turn mutex — see
// tasks.rs's TaskPhase::Pending doc comment. Followed by "started" once the
// batch actually begins.
export type TaskOrigin = "manual" | "scheduler" | "background";

export interface TaskProgress {
  percentDone?: number;
  itemsDone?: number;
  itemsTotal?: number;
  bytesDone?: number;
  bytesTotal?: number;
  label?: string;
  // Per-kind detail, kept lossless vs the legacy *:progress events even though
  // no consumer reads these yet — see tasks.rs's TaskProgress doc comment.
  secondsElapsed?: number;
  secondsRemaining?: number;
  currentFiles?: string[];
  repoId?: string;
}

export interface TaskEvent {
  operationId: string;
  kind: TaskKind;
  phase: TaskPhase;
  repoId: string;
  targetId?: string;
  origin: TaskOrigin;
  progress?: TaskProgress;
  error?: string;
  at: number;
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
