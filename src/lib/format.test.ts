import { describe, it, expect, vi, afterEach } from "vitest";
import { formatBytes, formatSize, formatDate, formatDateOnly, formatTimestamp, formatDuration, formatRelative, formatRepoSize, isOverdue } from "./format";
import type { ResticStats } from "./types";

describe("formatBytes", () => {
  it("returns '0 B' for zero", () => {
    expect(formatBytes(0)).toBe("0 B");
  });

  it("formats bytes below 1 KB", () => {
    expect(formatBytes(1)).toBe("1 B");
    expect(formatBytes(1023)).toBe("1023 B");
  });

  it("formats kilobytes", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1536)).toBe("1.5 KB");
  });

  it("formats megabytes", () => {
    expect(formatBytes(1024 ** 2)).toBe("1.0 MB");
    expect(formatBytes(1.5 * 1024 ** 2)).toBe("1.5 MB");
  });

  it("formats gigabytes with two decimal places", () => {
    expect(formatBytes(1024 ** 3)).toBe("1.00 GB");
    expect(formatBytes(2.5 * 1024 ** 3)).toBe("2.50 GB");
  });

  it("formats terabytes with two decimal places", () => {
    expect(formatBytes(1024 ** 4)).toBe("1.00 TB");
    expect(formatBytes(1.25 * 1024 ** 4)).toBe("1.25 TB");
  });
});

describe("formatSize", () => {
  it("returns em dash for undefined", () => {
    expect(formatSize(undefined)).toBe("—");
  });

  it("returns em dash for zero", () => {
    expect(formatSize(0)).toBe("—");
  });

  it("delegates to formatBytes for non-zero values", () => {
    expect(formatSize(1024)).toBe("1.0 KB");
  });
});

// format.ts's dateTimeFormatter/dateOnlyFormatter are module-level Intl.DateTimeFormat
// instances with no explicit timeZone or locale, so their output depends on whatever timezone
// AND locale the process resolves at import time — both pinned (TZ/LANG/LC_ALL) via
// vite.config.ts's test.env so these assertions are real expected strings, not the
// toBeTruthy()/self-equality checks this file used to fall back to (which would pass for any
// implementation, including a wrong one).
describe("formatDate", () => {
  it("formats a Unix-seconds timestamp", () => {
    expect(formatDate(0)).toBe("01/01/1970, 12:00:00 AM");
  });

  it("formats an ISO string identically to the equivalent Unix-seconds value", () => {
    expect(formatDate("2024-01-15T12:00:00Z")).toBe("01/15/2024, 12:00:00 PM");
    expect(formatDate(1705320000)).toBe("01/15/2024, 12:00:00 PM");
  });
});

describe("formatDateOnly", () => {
  it("formats an ISO string as a date with no time component", () => {
    expect(formatDateOnly("2024-01-15T12:00:00Z")).toBe("1/15/2024");
  });
});

describe("formatTimestamp", () => {
  it("returns 'Never' for undefined", () => {
    expect(formatTimestamp(undefined)).toBe("Never");
  });

  it("returns 'Never' for zero", () => {
    expect(formatTimestamp(0)).toBe("Never");
  });

  it("delegates to formatDate for non-zero values", () => {
    expect(formatTimestamp(1705320000)).toBe(formatDate(1705320000));
  });
});

describe("formatDuration", () => {
  it("formats seconds under a minute as integer by default", () => {
    expect(formatDuration(0)).toBe("0s");
    expect(formatDuration(45)).toBe("45s");
    expect(formatDuration(59)).toBe("59s");
  });

  it("formats seconds with fractional when flag is set", () => {
    expect(formatDuration(5.7, true)).toBe("5.7s");
    expect(formatDuration(0, true)).toBe("0.0s");
  });

  it("floors fractional seconds when flag is false", () => {
    expect(formatDuration(45.9)).toBe("45s");
  });

  it("formats minutes and seconds", () => {
    expect(formatDuration(60)).toBe("1m 0s");
    expect(formatDuration(125)).toBe("2m 5s");
    expect(formatDuration(3599)).toBe("59m 59s");
  });

  it("formats hours and minutes", () => {
    expect(formatDuration(3600)).toBe("1h 0m");
    expect(formatDuration(3661)).toBe("1h 1m");
    expect(formatDuration(7322)).toBe("2h 2m");
  });
});

describe("formatRelative", () => {
  const now = Math.floor(Date.now() / 1000);

  it("formats near-future timestamps as 'in under a minute'", () => {
    expect(formatRelative(now + 30)).toBe("in under a minute");
  });

  it("formats future minutes/hours/days", () => {
    expect(formatRelative(now + 3 * 60)).toBe("in 3 mins");
    expect(formatRelative(now + 60)).toBe("in 1 min");
    expect(formatRelative(now + 3 * 3600)).toBe("in 3 hours");
    expect(formatRelative(now + 3600)).toBe("in 1 hour");
    expect(formatRelative(now + 4 * 86400)).toBe("in 4 days");
  });

  it("formats past timestamps as '... ago'", () => {
    expect(formatRelative(now - 30)).toBe("just now");
    expect(formatRelative(now - 10 * 60)).toBe("10 mins ago");
    expect(formatRelative(now - 3600)).toBe("1 hour ago");
  });
});

describe("formatRepoSize", () => {
  it("falls back to the single-size layout when raw_size is absent (legacy cache row)", () => {
    const stats: ResticStats = { total_size: 1024 ** 3, total_file_count: 10, snapshots_count: 3 };
    const result = formatRepoSize(stats);
    expect(result.primary).toBe("1.00 GB");
    expect(result.secondary).toBe("3 snapshots");
    expect(result.tooltip).toBeUndefined();
  });

  it("falls back to the single-size layout when raw_size is explicitly null", () => {
    const stats: ResticStats = { total_size: 1024 ** 3, total_file_count: 10, snapshots_count: 1, raw_size: null };
    const result = formatRepoSize(stats);
    expect(result.primary).toBe("1.00 GB");
    expect(result.secondary).toBe("1 snapshot");
    expect(result.tooltip).toBeUndefined();
  });

  it("promotes raw_size to primary and folds total_size + snapshot count into secondary", () => {
    const stats: ResticStats = {
      total_size: 4 * 1024 ** 3,
      total_file_count: 100,
      snapshots_count: 12,
      raw_size: 1024 ** 3,
    };
    const result = formatRepoSize(stats);
    expect(result.primary).toBe("1.00 GB");
    expect(result.secondary).toBe("4.00 GB · 12 snapshots");
    expect(result.tooltip).toBe("Stored 1.00 GB of 4.00 GB restorable — 75% saved by compression + deduplication.");
  });

  it("treats a zero total_size as 0% saving rather than dividing by zero", () => {
    const stats: ResticStats = { total_size: 0, total_file_count: 0, snapshots_count: 0, raw_size: 0 };
    const result = formatRepoSize(stats);
    expect(result.tooltip).toContain("0% saved");
  });

  it("clamps a negative saving (raw_size > total_size) to 0%", () => {
    const stats: ResticStats = {
      total_size: 100,
      total_file_count: 1,
      snapshots_count: 1,
      raw_size: 150,
    };
    const result = formatRepoSize(stats);
    expect(result.tooltip).toContain("0% saved");
  });
});

describe("isOverdue", () => {
  const NOW = 1_700_000_000;

  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns false for undefined", () => {
    expect(isOverdue(undefined)).toBe(false);
  });

  it("returns false for 0 (falsy — no schedule should ever read as overdue before it exists)", () => {
    expect(isOverdue(0)).toBe(false);
  });

  it("returns false for a timestamp in the future", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW * 1000);
    expect(isOverdue(NOW + 60)).toBe(false);
  });

  it("returns true for a timestamp in the past", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW * 1000);
    expect(isOverdue(NOW - 60)).toBe(true);
  });

  it("returns true for exactly now (<=, not <)", () => {
    vi.useFakeTimers();
    vi.setSystemTime(NOW * 1000);
    expect(isOverdue(NOW)).toBe(true);
  });
});
