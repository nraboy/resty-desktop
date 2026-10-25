import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { checkFullDiskAccess, openFullDiskAccessSettings } from "../../lib/invoke";
import type { FullDiskAccessStatus } from "../../lib/invoke";
import { activeFilesFrom, activePaths, effectiveTags, type SourceMode } from "../../lib/planForm";
import { needsFullDiskAccess, planReadPaths } from "../../lib/utils";
import Button from "../Button";
import SegmentedToggle from "../SegmentedToggle";
import { CARD, type FormProps } from "./types";

/** Source paths OR path-list files (`--files-from`) — either/or in the editor. */
export default function SourcesCard({ form, update }: FormProps) {
  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);
  useEffect(() => {
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
  }, []);

  const pick = useCallback(
    async (opts: { directory?: boolean }, apply: (picked: string[]) => void) => {
      const selected = await open({ ...opts, multiple: true });
      if (!selected) return;
      apply(Array.isArray(selected) ? selected : [selected]);
    },
    [],
  );
  const addPaths = (arr: string[]) => update((p) => ({ paths: [...new Set([...p.paths, ...arr])] }));
  const addFilesFrom = (arr: string[]) => update((p) => ({ filesFrom: [...new Set([...p.filesFrom, ...arr])] }));

  const { paths, filesFrom, sourceMode } = form;
  return (
    <div className={CARD}>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-medium text-gray-300">Sources</h2>
        <SegmentedToggle<SourceMode>
          value={sourceMode}
          onChange={(m) => update({ sourceMode: m })}
          options={[
            { value: "paths", label: "Paths" },
            { value: "files", label: "List files" },
          ]}
        />
      </div>

      {paths.length > 0 && filesFrom.length > 0 && (
        <p className="text-xs text-amber-400 mb-3">
          Source paths and path-list files are both set. Only the active tab's entries are kept
          when you save.
        </p>
      )}

      {sourceMode === "paths" ? (
        <>
          <div className="flex justify-end gap-2 mb-3">
            <Button variant="ghost" size="sm" onClick={() => pick({}, addPaths)}>+ Files</Button>
            <Button variant="secondary" size="sm" onClick={() => pick({ directory: true }, addPaths)}>+ Folder</Button>
          </div>

          {paths.length === 0 ? (
            <p className="text-sm text-gray-500 text-center py-4">
              No paths selected. Add a file or folder to back up.
            </p>
          ) : (
            <ul className="space-y-1.5">
              {paths.map((p) => (
                <li key={p} className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2">
                  <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                  <button
                    onClick={() => update((prev) => ({ paths: prev.paths.filter((x) => x !== p) }))}
                    className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <>
          <p className="text-xs text-gray-500 mb-3">
            Text files listing what to back up, one path per line, passed to restic as{" "}
            <code className="text-gray-400">--files-from</code>. Lines starting with{" "}
            <code className="text-gray-400">#</code> are comments and glob patterns are
            expanded. Use absolute paths inside the file. restic reads it at backup time, so
            edits apply to the next run, and a missing file fails the backup.
          </p>
          <div className="flex justify-end mb-3">
            <Button variant="secondary" size="sm" onClick={() => pick({}, addFilesFrom)}>+ File</Button>
          </div>

          {filesFrom.length === 0 ? (
            <p className="text-sm text-gray-500 text-center py-4">
              No path-list files. Add a text file listing the paths to back up.
            </p>
          ) : (
            <ul className="space-y-1.5">
              {filesFrom.map((p) => (
                <li key={p} className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2">
                  <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                  <button
                    onClick={() => update((prev) => ({ filesFrom: prev.filesFrom.filter((x) => x !== p) }))}
                    className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}

          {effectiveTags(form).length === 0 && (
            <p className="text-xs text-amber-400 mt-3">
              Plans that use path-list files need at least one tag (on the Tags tab), since their
              snapshots' paths come from the list files and can't be matched any other way.
            </p>
          )}
        </>
      )}

      {planReadPaths({
        paths: activePaths(form),
        filesFrom: activeFilesFrom(form),
        excludeFiles: form.excludeFiles,
      }).some(needsFullDiskAccess) &&
        fdaStatus?.supported &&
        !fdaStatus.granted && (
          <div className="mt-3 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg text-xs text-amber-300">
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
        )}
    </div>
  );
}
