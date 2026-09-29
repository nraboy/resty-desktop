import { MAX_PACK_SIZE_MIB, MIN_PACK_SIZE_MIB } from "./config";
import type { BackupPlan } from "./types";

export function needsFullDiskAccess(p: string): boolean {
  return (
    /\/Library(\/|$)/.test(p) ||
    p === "/System" || p.startsWith("/System/") ||
    p === "/private" || p.startsWith("/private/") ||
    p === "/var" || p.startsWith("/var/")
  );
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
