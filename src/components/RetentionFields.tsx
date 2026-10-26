import type { RetentionFields as RetentionFieldValues } from "../lib/utils";

const ROWS: { key: keyof RetentionFieldValues; label: string; unit: string }[] = [
  { key: "keepLast", label: "Keep last", unit: "snapshots" },
  { key: "keepDaily", label: "Keep daily", unit: "days" },
  { key: "keepWeekly", label: "Keep weekly", unit: "weeks" },
  { key: "keepMonthly", label: "Keep monthly", unit: "months" },
  { key: "keepYearly", label: "Keep yearly", unit: "years" },
];

/** The five `keep_*` inputs, shared by the plan editor and Settings' default retention card so the
 *  two can't drift apart. Blank = unset; validation lives in `parseRetention` (lib/utils.ts). */
export default function RetentionFields({
  value,
  onChange,
}: {
  value: RetentionFieldValues;
  onChange: (next: RetentionFieldValues) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-x-6 gap-y-3">
      {ROWS.map(({ key, label, unit }) => (
        <div key={key} className="flex items-center gap-3">
          <label className="text-xs text-gray-400 w-28 flex-shrink-0">{label}</label>
          <input
            type="number"
            min="1"
            step="1"
            // Whole numbers only (restic's --keep-* are integers): block the characters a number
            // input would otherwise accept for decimals, exponents and signs.
            onKeyDown={(e) => { if ([".", ",", "e", "E", "+", "-"].includes(e.key)) e.preventDefault(); }}
            value={value[key]}
            onChange={(e) => onChange({ ...value, [key]: e.target.value })}
            placeholder="—"
            className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
          />
          <span className="text-xs text-gray-500">{unit}</span>
        </div>
      ))}
    </div>
  );
}
