import { useEffect, useState } from "react";
import { checkFullDiskAccess, openFullDiskAccessSettings } from "../../lib/invoke";
import type { FullDiskAccessStatus } from "../../lib/invoke";
import { needsFullDiskAccess, planReadPaths } from "../../lib/utils";
import type { FormProps } from "./types";

/** macOS only: warns when a source (or a file restic reads for the plan) sits in a protected
 * location and Full Disk Access hasn't been granted. */
export default function FdaNotice({ form }: Pick<FormProps, "form">) {
  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);
  useEffect(() => {
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
  }, []);

  const needed = planReadPaths({
    paths: form.paths,
    filesFrom: form.filesFrom,
    excludeFiles: form.excludeFiles,
  }).some(needsFullDiskAccess);
  if (!needed || !fdaStatus?.supported || fdaStatus.granted) return null;

  return (
    <div className="mb-4 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg text-xs text-amber-300">
      <span className="font-medium">Full Disk Access may be required.</span>{" "}
      One or more paths (e.g. <code className="text-amber-300">~/Library</code>, system directories) are protected by macOS and cannot be read without Full Disk Access. Go to{" "}
      <span className="font-medium">System Settings → Privacy &amp; Security → Full Disk Access</span>{" "}
      and add Resty Desktop to avoid permission errors.
      <button
        type="button"
        onClick={() => openFullDiskAccessSettings().catch(() => {})}
        className="mt-2 flex items-center gap-1 text-amber-300 hover:text-amber-400 underline underline-offset-2 transition-colors"
      >
        Open Full Disk Access Settings
        <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M10 6H6a2 2 0 00-2 2v10a2 2 0 002 2h10a2 2 0 002-2v-4M14 4h6m0 0v6m0-6L10 14" />
        </svg>
      </button>
    </div>
  );
}
