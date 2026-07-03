// Shared display formatters used across pages.

/** Human-readable byte size (e.g. "1.5 MB"). */
export function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes < 1024 ** 4) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  return `${(bytes / 1024 ** 4).toFixed(2)} TB`;
}

/** Like formatBytes but renders missing/zero sizes as an em dash. */
export function formatSize(bytes?: number): string {
  if (!bytes) return "—";
  return formatBytes(bytes);
}

// Constructing an Intl.DateTimeFormat is relatively expensive; formatDate is called
// once per row in search result lists (up to 200 rows) that re-render on every
// keystroke/debounce, so a single module-level formatter is reused across calls.
const dateTimeFormatter = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

/** Format an ISO string or a Unix-seconds timestamp as a locale date-time, zero-padded (e.g. "06/01/2026, 03:45:12 PM"). */
export function formatDate(value: string | number): string {
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  return dateTimeFormatter.format(date);
}

const dateOnlyFormatter = new Intl.DateTimeFormat(undefined);

/** Format an ISO string as a locale date (no time), e.g. for file-browser mtimes. */
export function formatDateOnly(value: string): string {
  return dateOnlyFormatter.format(new Date(value));
}

/** Like formatDate for Unix-seconds, but renders missing values as "Never". */
export function formatTimestamp(ts?: number): string {
  if (!ts) return "Never";
  return formatDate(ts);
}

/** Human-readable duration in seconds (e.g. "2m 5s"). */
export function formatDuration(secs: number, fractional = false): string {
  if (secs < 60) return fractional ? `${secs.toFixed(1)}s` : `${Math.floor(secs)}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${Math.floor(secs % 60)}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
