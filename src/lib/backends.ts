// Backend detection for the repository add/edit forms — mirrors
// src-tauri/src/commands/backends.rs's detect_kind. The repository path is always
// freeform (typed directly or picked via folder browser); this module only powers a
// one-line hint naming common credential env vars once a known prefix is recognized.
// Actual credential validation is Rust-side (backends.rs's validate_credentials),
// authoritative and run on every save — this is display-only.

export type BackendKind = "local" | "s3" | "b2" | "other";

/** Same prefix list as isRemoteRepo (types.ts) — kept in sync manually. */
const REMOTE_PREFIXES = ["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

/** Mirrors backends.rs's detect_kind — total, so it never fails and there is
 * nothing to migrate for a repo added before this feature. */
export function detectBackend(path: string): BackendKind {
  if (path.startsWith("s3:")) return "s3";
  if (path.startsWith("b2:")) return "b2";
  if (REMOTE_PREFIXES.some((p) => path.startsWith(p))) return "other";
  return "local";
}

const COMMON_CREDENTIAL_KEYS: Partial<Record<BackendKind, string>> = {
  s3: "AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY",
  b2: "B2_ACCOUNT_ID, B2_ACCOUNT_KEY",
};

/** One-line hint naming the common env vars for a detected backend, or undefined for
 * local/other/unrecognized paths — there's nothing generic to suggest for those. */
export function commonCredentialKeys(path: string): string | undefined {
  return COMMON_CREDENTIAL_KEYS[detectBackend(path)];
}
