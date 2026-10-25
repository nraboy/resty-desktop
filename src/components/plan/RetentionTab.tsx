import { CARD, type FormProps } from "./types";

export default function RetentionTab({ form, update }: FormProps) {
  const fields = [
    { key: "keepLast", label: "Keep last", unit: "snapshots" },
    { key: "keepDaily", label: "Keep daily", unit: "days" },
    { key: "keepWeekly", label: "Keep weekly", unit: "weeks" },
    { key: "keepMonthly", label: "Keep monthly", unit: "months" },
    { key: "keepYearly", label: "Keep yearly", unit: "years" },
  ] as const;
  return (
    <div className={CARD}>
      <h2 className="text-sm font-medium text-gray-300 mb-1">Retention Policy (optional)</h2>
      <p className="text-xs text-gray-500 mb-4">
        After each backup, old snapshots will be pruned. Leave all fields blank to skip pruning.
      </p>
      <div className="grid grid-cols-2 gap-x-6 gap-y-3">
        {fields.map(({ key, label, unit }) => (
          <div key={key} className="flex items-center gap-3">
            <label htmlFor={`retention-${key}`} className="text-xs text-gray-400 w-28 flex-shrink-0">{label}</label>
            <input
              id={`retention-${key}`}
              type="number"
              min="0"
              value={form[key]}
              onChange={(e) => update({ [key]: e.target.value })}
              placeholder="—"
              className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
            />
            <span className="text-xs text-gray-500">{unit}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
