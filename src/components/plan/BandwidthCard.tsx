import { CARD, NUMBER_INPUT, type FormProps } from "./types";

/** The Settings tab's Bandwidth section: upload/download limits (its title is the section heading). */
export default function BandwidthCard({ form, update }: FormProps) {
  return (
    <div className={CARD}>
      <p className="text-xs text-gray-500 mb-4">
        Limits are in KiB/s. Leave blank for unlimited. These settings only affect remote repositories (S3, SFTP, etc.) — they have no effect on local repos.
      </p>
      <div className="grid grid-cols-2 gap-x-6 gap-y-3">
        {(
          [
            { key: "limitUpload", label: "Upload limit" },
            { key: "limitDownload", label: "Download limit" },
          ] as const
        ).map(({ key, label }) => (
          <div key={key} className="flex items-center gap-3">
            <label htmlFor={`limit-${key}`} className="text-xs text-gray-400 w-28 flex-shrink-0">{label}</label>
            <input
              id={`limit-${key}`}
              type="number"
              min="0"
              value={form[key]}
              onChange={(e) => update({ [key]: e.target.value })}
              placeholder="—"
              className={NUMBER_INPUT}
            />
            <span className="text-xs text-gray-500">KiB/s</span>
          </div>
        ))}
      </div>
    </div>
  );
}
