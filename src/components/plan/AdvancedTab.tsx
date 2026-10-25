import { MAX_PACK_SIZE_MIB, MIN_PACK_SIZE_MIB } from "../../lib/config";
import { isWindows } from "../../lib/utils";
import WebhooksCard from "./WebhooksCard";
import { CARD, type FormProps } from "./types";

const NUMBER_INPUT =
  "w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500";

/** Bandwidth limits, restic options (pack size, Windows-only flags) and webhooks. */
export default function AdvancedTab({ form, update }: FormProps) {
  const onWindows = isWindows();
  const { excludeCloudFiles, useFsSnapshot } = form;
  return (
    <>
      <div className={CARD}>
        <h2 className="text-sm font-medium text-gray-300 mb-1">Bandwidth Limits (optional)</h2>
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

      <div className={CARD}>
        <h2 className="text-sm font-medium text-gray-300 mb-1">Restic options (optional)</h2>
        <p className="text-xs text-gray-500 mb-4">
          Extra <code className="text-gray-400">restic backup</code> options for this plan.
        </p>

        <div className="flex items-center gap-3">
          <label htmlFor="pack-size" className="text-xs text-gray-400 w-28 flex-shrink-0">Pack size</label>
          <input
            id="pack-size"
            type="number"
            min={MIN_PACK_SIZE_MIB}
            max={MAX_PACK_SIZE_MIB}
            step="1"
            value={form.packSize}
            onChange={(e) => update({ packSize: e.target.value })}
            placeholder="16"
            className={NUMBER_INPUT}
          />
          <span className="text-xs text-gray-500">MiB</span>
        </div>
        <p className="text-xs text-gray-500 mt-2">
          Size of the data files restic writes ({MIN_PACK_SIZE_MIB}–{MAX_PACK_SIZE_MIB}). Larger packs (e.g. 64) mean
          fewer objects on cloud storage. Leave blank for restic&apos;s default (16 MiB). Applies to
          backups from this plan only — prune and mirror use restic&apos;s default.
        </p>

        {onWindows && (
          <div className="mt-3 pt-3 border-t border-gray-800 space-y-3">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={excludeCloudFiles}
                onChange={(e) => update({ excludeCloudFiles: e.target.checked })}
                className="rounded bg-gray-700 border-gray-600"
              />
              <span className="text-sm text-gray-300">
                Skip cloud-only placeholder files, e.g. OneDrive (<code className="text-gray-400">--exclude-cloud-files</code>)
              </span>
            </label>
            <div>
              <label className="flex items-center gap-2">
                <input
                  type="checkbox"
                  checked={useFsSnapshot}
                  onChange={(e) => update({ useFsSnapshot: e.target.checked })}
                  className="rounded bg-gray-700 border-gray-600"
                />
                <span className="text-sm text-gray-300">
                  Use a Volume Shadow Copy (VSS) snapshot (<code className="text-gray-400">--use-fs-snapshot</code>)
                </span>
              </label>
              <p className="text-xs text-amber-400 mt-1 ml-6">
                Requires running Resty Desktop as Administrator. Without it, restic can&apos;t create the snapshot.
              </p>
            </div>
          </div>
        )}

        {!onWindows && (excludeCloudFiles || useFsSnapshot) && (
          <p className="text-xs text-gray-500 mt-3 pt-3 border-t border-gray-800">
            This plan has Windows-only options enabled (
            {[excludeCloudFiles && "skip cloud-only files", useFsSnapshot && "VSS snapshot"].filter(Boolean).join(", ")}
            ). They&apos;re kept but ignored when backing up on this computer.
          </p>
        )}
      </div>

      <WebhooksCard form={form} update={update} />
    </>
  );
}
