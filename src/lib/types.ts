export interface Repository {
  id: string;
  name: string;
  path: string;
  password: string;
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
