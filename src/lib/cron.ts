// Pure cron helpers shared by ScheduleEditPage.tsx's Simple/Expert mode toggle. Moved out of
// the page (where they were module-private) so they're directly unit-testable — see
// cron.test.ts.
import type { ScheduleFrequency } from "./types";

export type SimpleFields = {
  frequency: ScheduleFrequency;
  hour: string;
  minute: string;
  dayOfWeek: string;
  dayOfMonth: string;
};

/** Parses a 5-field cron expression into the Simple-mode form fields, or `null` if it doesn't
 *  match one of the four recognized shapes (hourly/daily/weekly/monthly) — the page falls back
 *  to Expert mode in that case. */
export function parseCronToSimple(expr: string): SimpleFields | null {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [m, h, dom, , dow] = parts;
  if (h === "*" && dom === "*" && dow === "*") {
    return { frequency: "hourly", hour: "2", minute: m, dayOfWeek: "1", dayOfMonth: "1" };
  }
  if (dow !== "*" && dom === "*") {
    return { frequency: "weekly", hour: h, minute: m, dayOfWeek: dow, dayOfMonth: "1" };
  }
  if (dom !== "*" && dow === "*") {
    return { frequency: "monthly", hour: h, minute: m, dayOfWeek: "1", dayOfMonth: dom };
  }
  if (dom === "*" && dow === "*") {
    return { frequency: "daily", hour: h, minute: m, dayOfWeek: "1", dayOfMonth: "1" };
  }
  return null;
}

/** Builds a 5-field cron expression from Simple-mode form fields. `hour`/`minute` are
 *  zero-padded — round-tripping a parsed expression through this is not byte-identical (e.g.
 *  "0 2 * * *" comes back as "00 02 * * *"), which is intended: cron accepts both, and the
 *  padded form is what the Simple-mode number inputs actually produce. */
export function buildCronExpr(
  frequency: ScheduleFrequency,
  hour: string,
  minute: string,
  dayOfWeek: string,
  dayOfMonth: string,
): string {
  const h = hour.padStart(2, "0");
  const m = minute.padStart(2, "0");
  switch (frequency) {
    case "hourly":
      return `${m} * * * *`;
    case "daily":
      return `${m} ${h} * * *`;
    case "weekly":
      return `${m} ${h} * * ${dayOfWeek}`;
    case "monthly":
      return `${m} ${h} ${dayOfMonth} * *`;
    default:
      return "";
  }
}
