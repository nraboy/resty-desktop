// Backend detection for the repository add/edit forms — mirrors
// src-tauri/src/commands/backends.rs's detect_kind. The repository path is always
// freeform (typed directly or picked via folder browser); this module only powers a
// one-line hint naming common credential env vars once a known prefix is recognized.
// Actual credential validation is Rust-side (backends.rs's validate_credentials),
// authoritative and run on every save — this is display-only.

export type BackendKind = "local" | "s3" | "b2" | "rest" | "other";

/** Same prefix list as isRemoteRepo (types.ts) — kept in sync manually. "rest:" stays
 * listed here even though detectBackend below now matches it in its own dedicated
 * check first: this list also backs remote_auto_refresh gating (isRemoteRepo) and
 * the cache warmer / SnapshotsPage background refresh, not just detectBackend's
 * dispatch. Removing it would silently stop treating REST repos as remote. */
const REMOTE_PREFIXES = ["s3:", "sftp:", "rest:", "azure:", "gs:", "b2:", "rclone:"];

/** Mirrors backends.rs's detect_kind — total, so it never fails and there is
 * nothing to migrate for a repo added before this feature. */
export function detectBackend(path: string): BackendKind {
  if (path.startsWith("s3:")) return "s3";
  if (path.startsWith("b2:")) return "b2";
  if (path.startsWith("rest:")) return "rest";
  if (REMOTE_PREFIXES.some((p) => path.startsWith(p))) return "other";
  return "local";
}

const COMMON_CREDENTIAL_KEYS: Partial<Record<BackendKind, string>> = {
  s3: "AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY",
  b2: "B2_ACCOUNT_ID, B2_ACCOUNT_KEY",
  rest: "RESTIC_REST_USERNAME, RESTIC_REST_PASSWORD",
};

/** One-line hint naming the common env vars for a detected backend, or undefined for
 * local/other/unrecognized paths — there's nothing generic to suggest for those. */
export function commonCredentialKeys(path: string): string | undefined {
  return COMMON_CREDENTIAL_KEYS[detectBackend(path)];
}

/** Splits a `rest:` path into the indexes the two checks below need, or null when
 * the path isn't a rest: URL. Deliberately not `new URL()` — that throws on the
 * exact malformed input we're trying to detect, and on partial typing. */
function restUserinfoParts(path: string): { firstSlash: number; lastAt: number } | null {
  if (!path.startsWith("rest:")) return null;
  let rest = path.slice("rest:".length);
  // Match the scheme case-insensitively — Go's net/url (and restic) treats
  // "HTTPS://"/"Http://" the same as "https://"/"http://", so a differently-cased
  // scheme must still be stripped or its own "//" gets mistaken for the userinfo
  // boundary below.
  const lowerRest = rest.toLowerCase();
  if (lowerRest.startsWith("https://")) rest = rest.slice("https://".length);
  else if (lowerRest.startsWith("http://")) rest = rest.slice("http://".length);

  const slashIdx = rest.indexOf("/");
  const queryIdx = rest.indexOf("?");
  const hashIdx = rest.indexOf("#");
  const candidates = [slashIdx, queryIdx, hashIdx].filter((i) => i !== -1);
  const firstSlash = candidates.length > 0 ? Math.min(...candidates) : -1;
  // > 0, not !== -1: an "@" with nothing before it (e.g. "rest:https://@host/") has
  // no actual username or password — restic's ApplyEnvironment still reads the env
  // vars for that case (empty username, no password set) — so it must not count as
  // "inline userinfo present" for either check below.
  const lastAt = rest.lastIndexOf("@");

  return { firstSlash, lastAt };
}

/** True when a `rest:` path carries inline userinfo that Go's net/url will mis-parse:
 * the authority ends at the first `/`, so a `/` in the password makes the remainder
 * look like a port ("invalid port \":pass\" after host"). Advisory only — restic is
 * the actual parser; this is a best-effort heuristic for a form hint. */
export function hasBrokenRestUserinfo(path: string): boolean {
  const parts = restUserinfoParts(path);
  if (!parts) return false;
  return parts.lastAt > 0 && parts.firstSlash !== -1 && parts.lastAt > parts.firstSlash;
}

/** True when a `rest:` path carries *any* inline userinfo. restic's ApplyEnvironment
 * reads RESTIC_REST_USERNAME/PASSWORD only when the URL has neither a username nor a
 * password, so inline credentials silently override stored credential rows. */
export function hasInlineRestUserinfo(path: string): boolean {
  const parts = restUserinfoParts(path);
  if (!parts) return false;
  return parts.lastAt > 0;
}
