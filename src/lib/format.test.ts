import { describe, it, expect } from "vitest";
import { formatBytes, formatSize, formatDate, formatTimestamp, formatDuration } from "./format";

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

describe("formatDate", () => {
  it("accepts a Unix-seconds number", () => {
    // 0 seconds = 1970-01-01T00:00:00.000Z; just verify it doesn't throw
    // and returns a non-empty string.
    expect(formatDate(0)).toBeTruthy();
  });

  it("accepts an ISO string", () => {
    expect(formatDate("2024-01-15T12:00:00Z")).toBeTruthy();
  });

  it("produces consistent output for same input", () => {
    expect(formatDate(1705320000)).toBe(formatDate(1705320000));
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
