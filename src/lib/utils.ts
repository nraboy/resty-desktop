import { MAX_PACK_SIZE_MIB, MIN_PACK_SIZE_MIB } from "./config";
import type { BackupPlan, RetentionPolicy } from "./types";

export function needsFullDiskAccess(p: string): boolean {
  return (
    /\/Library(\/|$)/.test(p) ||
    p === "/System" || p.startsWith("/System/") ||
    p === "/private" || p.startsWith("/private/") ||
    p === "/var" || p.startsWith("/var/")
  );
}

/** True when a restic warning message is a permission problem (Unix and Windows wording). */
export function isPermissionWarning(message: string): boolean {
  return /permission denied|operation not permitted|access is denied/i.test(message);
}

/** Every local file restic must read for this plan: source paths plus both list-file kinds. */
export function planReadPaths(
  plan: Pick<BackupPlan, "paths" | "filesFrom" | "excludeFiles">,
): string[] {
  return [...plan.paths, ...(plan.filesFrom ?? []), ...(plan.excludeFiles ?? []).map((f) => f.path)];
}

/** True for a Windows WebView2 user-agent (WKWebView reports "Macintosh", WebKitGTK "X11; Linux"). */
export function isWindowsUserAgent(ua: string): boolean {
  return /\bWindows\b/.test(ua);
}

/** Whether this app instance is running on Windows — gates the Windows-only backup options. */
export function isWindows(): boolean {
  return typeof navigator !== "undefined" && isWindowsUserAgent(navigator.userAgent);
}

/**
 * Parses the pack-size field (MiB). Blank = restic's default (`value: undefined`). Whole
 * numbers only — `parseInt` alone would silently truncate "12.5" or "1e2" — within restic's
 * accepted 4–128 range. Surrounding whitespace is ignored.
 */
export function parsePackSize(input: string): { value: number | undefined } | { error: string } {
  const s = input.trim();
  if (s === "") return { value: undefined };
  const range = `Pack size must be a whole number between ${MIN_PACK_SIZE_MIB} and ${MAX_PACK_SIZE_MIB} MiB.`;
  if (!/^\d+$/.test(s)) return { error: range };
  const n = Number(s);
  if (n < MIN_PACK_SIZE_MIB || n > MAX_PACK_SIZE_MIB) return { error: range };
  return { value: n };
}

/** The five retention inputs as the strings the user typed (blank = unset). */
export type RetentionFields = {
  keepLast: string;
  keepDaily: string;
  keepWeekly: string;
  keepMonthly: string;
  keepYearly: string;
};

export const EMPTY_RETENTION_FIELDS: RetentionFields = {
  keepLast: "",
  keepDaily: "",
  keepWeekly: "",
  keepMonthly: "",
  keepYearly: "",
};

export function retentionToFields(r?: RetentionPolicy): RetentionFields {
  return {
    keepLast: r?.keepLast?.toString() ?? "",
    keepDaily: r?.keepDaily?.toString() ?? "",
    keepWeekly: r?.keepWeekly?.toString() ?? "",
    keepMonthly: r?.keepMonthly?.toString() ?? "",
    keepYearly: r?.keepYearly?.toString() ?? "",
  };
}

/**
 * Turns the retention inputs into a policy. All blank → `{ value: undefined }` (no pruning).
 * 0 means "unset" throughout the app (restic rejects a `forget` with no effective policy), so a
 * 0, negative, decimal or non-numeric count is an error rather than a silently dead rule.
 */
export function parseRetention(f: RetentionFields): { value: RetentionPolicy | undefined } | { error: string } {
  const entries = Object.values(f);
  if (entries.every((s) => s.trim() === "")) return { value: undefined };
  // Digits only: restic's --keep-* flags take integers, and `parseInt` alone would silently
  // truncate "12.5" or "1e2". A blank field is unset; anything else must be a whole number ≥ 1.
  const error = "Retention counts must be whole numbers of at least 1 — leave a field blank to disable it.";
  const toNum = (s: string): number | undefined | null => {
    const t = s.trim();
    if (t === "") return undefined;
    if (!/^\d+$/.test(t)) return null;
    const n = Number(t);
    return n < 1 ? null : n;
  };
  const parsed = {
    keepLast: toNum(f.keepLast),
    keepDaily: toNum(f.keepDaily),
    keepWeekly: toNum(f.keepWeekly),
    keepMonthly: toNum(f.keepMonthly),
    keepYearly: toNum(f.keepYearly),
  };
  if (Object.values(parsed).some((v) => v === null)) return { error };
  const value = parsed as RetentionPolicy;
  return { value };
}
