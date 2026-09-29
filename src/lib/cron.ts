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

const isPlainNum = (s: string) => /^\d+$/.test(s);

/** Parses a 5-field cron expression into the Simple-mode form fields, or `null` if it doesn't
 *  match one of the four recognized shapes (hourly/daily/weekly/monthly) — the page falls back
 *  to Expert mode in that case. Every bound field must be a plain integer and the month field
 *  a bare `*`: steps (every-N), lists (`15,45`), ranges (`1-5`), names and a restricted month
 *  are all things Simple mode can't represent, and accepting them would silently rewrite the
 *  expression the next time the form is touched. */
export function parseCronToSimple(expr: string): SimpleFields | null {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) return null;
  const [m, h, dom, month, dow] = parts;
  if (month !== "*" || !isPlainNum(m)) return null;
  if (h === "*" && dom === "*" && dow === "*") {
    return { frequency: "hourly", hour: "2", minute: m, dayOfWeek: "1", dayOfMonth: "1" };
  }
  if (!isPlainNum(h)) return null;
  if (dow !== "*" && dom === "*" && isPlainNum(dow)) {
    return { frequency: "weekly", hour: h, minute: m, dayOfWeek: dow, dayOfMonth: "1" };
  }
  if (dom !== "*" && dow === "*" && isPlainNum(dom)) {
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
