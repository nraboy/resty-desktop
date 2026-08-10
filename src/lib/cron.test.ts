import { describe, it, expect } from "vitest";
import { parseCronToSimple, buildCronExpr } from "./cron";

describe("parseCronToSimple", () => {
  it("returns null for a non-5-field expression", () => {
    expect(parseCronToSimple("0 0 * *")).toBeNull();
    expect(parseCronToSimple("0 0 * * * *")).toBeNull();
    expect(parseCronToSimple("")).toBeNull();
  });

  it("detects hourly (hour, day-of-month, and day-of-week all wildcarded)", () => {
    expect(parseCronToSimple("30 * * * *")).toEqual({
      frequency: "hourly",
      hour: "2",
      minute: "30",
      dayOfWeek: "1",
      dayOfMonth: "1",
    });
  });

  it("detects weekly (day-of-week set, day-of-month wildcarded)", () => {
    expect(parseCronToSimple("15 9 * * 3")).toEqual({
      frequency: "weekly",
      hour: "9",
      minute: "15",
      dayOfWeek: "3",
      dayOfMonth: "1",
    });
  });

  it("detects monthly (day-of-month set, day-of-week wildcarded)", () => {
    expect(parseCronToSimple("0 4 15 * *")).toEqual({
      frequency: "monthly",
      hour: "4",
      minute: "0",
      dayOfWeek: "1",
      dayOfMonth: "15",
    });
  });

  it("detects daily (day-of-month and day-of-week both wildcarded, hour set)", () => {
    expect(parseCronToSimple("0 2 * * *")).toEqual({
      frequency: "daily",
      hour: "2",
      minute: "0",
      dayOfWeek: "1",
      dayOfMonth: "1",
    });
  });

  it("returns null when both day-of-month and day-of-week are restricted (forces Expert mode)", () => {
    // None of the four recognized Simple-mode shapes match this — the page falls back to
    // Expert mode rather than guessing, matching describe_cron's identical fallthrough
    // (schedule.rs) for the same combination.
    expect(parseCronToSimple("0 0 15 * 1")).toBeNull();
  });
});

describe("buildCronExpr", () => {
  it("builds hourly", () => {
    expect(buildCronExpr("hourly", "2", "30", "1", "1")).toBe("30 * * * *");
  });

  it("builds daily", () => {
    expect(buildCronExpr("daily", "9", "0", "1", "1")).toBe("00 09 * * *");
  });

  it("builds weekly", () => {
    expect(buildCronExpr("weekly", "9", "15", "3", "1")).toBe("15 09 * * 3");
  });

  it("builds monthly", () => {
    expect(buildCronExpr("monthly", "4", "0", "1", "15")).toBe("00 04 15 * *");
  });

  it("returns an empty string for an unrecognized frequency", () => {
    expect(buildCronExpr("custom", "0", "0", "1", "1")).toBe("");
  });

  it("zero-pads single-digit hour and minute", () => {
    expect(buildCronExpr("daily", "2", "5", "1", "1")).toBe("05 02 * * *");
  });

  it("round-trips through parseCronToSimple with zero-padding, not byte-identically", () => {
    // Deliberately pinned, not a bug: cron accepts both "0 2 * * *" and "00 02 * * *", and the
    // padded form is what the Simple-mode number inputs actually produce. Don't "fix" this into
    // exact round-tripping.
    const parsed = parseCronToSimple("0 2 * * *")!;
    expect(parsed.hour).toBe("2");
    const rebuilt = buildCronExpr(parsed.frequency, parsed.hour, parsed.minute, parsed.dayOfWeek, parsed.dayOfMonth);
    expect(rebuilt).toBe("00 02 * * *");
    expect(rebuilt).not.toBe("0 2 * * *");
  });
});
