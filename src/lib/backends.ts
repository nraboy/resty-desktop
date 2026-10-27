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

/** True when `--insecure-tls` means something for this path: a `rest:https://` URL.
 * (restic ignores the flag for plain http, and the app only offers it for REST.) */
export function supportsInsecureTls(path: string): boolean {
  // The `rest:` prefix is case-sensitive (restic rejects `REST:` as an invalid backend, and
  // detect_kind treats it as a local path); the URL scheme after it is not.
  const p = path.trim();
  return p.startsWith("rest:") && /^https:\/\//i.test(p.slice("rest:".length));
}

/** Mirrors repo.rs's `verifies_tls`: transports where `--insecure-tls` would change the
 * security of a repo that doesn't have it set. */
function verifiesTls(path: string): boolean {
  const p = path.trim();
  return supportsInsecureTls(p) || ["s3:", "b2:", "azure:", "gs:"].some((prefix) => p.startsWith(prefix));
}

/** Mirrors repo.rs's `check_tls_compat`. restic's `--insecure-tls` is process-wide, so a
 * copy/mirror between a flagged repo and a TLS-verifying one that isn't flagged would
 * silently turn verification off for the second; the backend refuses it, and the pickers
 * use this to grey such targets out up front. */
export function insecureTlsConflict(
  a: { path: string; insecureTls: boolean },
  b: { path: string; insecureTls: boolean },
): boolean {
  const weakens = (flagged: typeof a, other: typeof a) =>
    flagged.insecureTls && !other.insecureTls && verifiesTls(other.path);
  return weakens(a, b) || weakens(b, a);
}

/** True when append-only means something for this path: a `rest:http(s)://` URL (it is a
 * rest-server option; no other backend has it). */
export function supportsAppendOnly(path: string): boolean {
  // `rest:` itself is case-sensitive (restic rejects `REST:`); the scheme after it is not.
  const p = path.trim();
  return p.startsWith("rest:") && /^https?:\/\//i.test(p.slice("rest:".length));
}

/** Tooltip / title for controls disabled because the repo is on an append-only server. */
export const APPEND_ONLY_TITLE =
  "This repository is on an append-only server — existing snapshots can't be deleted or modified";

/** Folds a detection result into the form: the new value for the append-only checkbox and a
 * line to append to the connection-test message ("" = nothing to add). The probe only ever
 * turns the checkbox on: an undetermined result, or one saying the server allows deletes,
 * leaves it alone, because a user may have ticked it for a reason the probe can't see (a proxy
 * or another server refusing deletes), and clearing it would re-enable forget/prune/tag that
 * then fail — tag even leaves a duplicate snapshot behind. */
export function applyAppendOnlyProbe(
  probe: { status: "appendOnly" | "notAppendOnly" | "undetermined"; detail: string },
  current: boolean,
): { appendOnly: boolean; note: string } {
  switch (probe.status) {
    case "appendOnly":
      return {
        appendOnly: true,
        note: current
          ? ""
          : "Append-only server detected: deleting or modifying snapshots (forget, prune, tag, retention) will be disabled for this repository.",
      };
    case "notAppendOnly":
      return {
        appendOnly: current,
        note: current
          ? "The server allows deletes, so it doesn't look append-only. The append-only setting was left on; untick it if that's no longer wanted."
          : "",
      };
    default:
      return { appendOnly: current, note: `Append-only check skipped: ${probe.detail}` };
  }
}

/** The two env vars restic reads for a `rest:` server's HTTP login. The add/edit dialogs show
 * them as dedicated "Server username" / "Server password" fields instead of generic rows. */
export const REST_USERNAME_KEY = "RESTIC_REST_USERNAME";
export const REST_PASSWORD_KEY = "RESTIC_REST_PASSWORD";

export function isRestLoginKey(key: string): boolean {
  const k = key.trim();
  return k === REST_USERNAME_KEY || k === REST_PASSWORD_KEY;
}

/** Value of the (first) credential row named `key`, or "" when there is none. */
export function getCredentialValue(rows: [string, string][], key: string): string {
  return rows.find(([k]) => k.trim() === key)?.[1] ?? "";
}

/** `rows` with `key` set to `value`: an existing row is updated in place, a missing one is
 * appended, and an emptied value removes the row (an unset login is the same as no row —
 * apply_backend_env skips empty values anyway). Other rows are untouched. */
export function setCredentialValue(
  rows: [string, string][],
  key: string,
  value: string,
): [string, string][] {
  const has = rows.some(([k]) => k.trim() === key);
  if (value === "") return rows.filter(([k]) => k.trim() !== key);
  if (has) return rows.map(([k, v]): [string, string] => (k.trim() === key ? [k, value] : [k, v]));
  return [...rows, [key, value]];
}
