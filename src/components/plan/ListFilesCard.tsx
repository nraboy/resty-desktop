import { open } from "@tauri-apps/plugin-dialog";
import { effectiveTags } from "../../lib/planForm";
import Button from "../Button";
import { CARD, type FormProps } from "./types";

/**
 * Path-list files (`restic backup --files-from`): text files that list more paths to back up, one
 * per line. They add to the folders picked in the tree rather than replacing them.
 */
export default function ListFilesCard({ form, update }: FormProps) {
  const { filesFrom } = form;

  const add = async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const picked = Array.isArray(selected) ? selected : [selected];
    update((p) => ({ filesFrom: [...new Set([...p.filesFrom, ...picked])] }));
  };

  return (
    <div className={CARD}>
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-sm font-medium text-gray-300">Path-list files</h2>
          <p className="text-xs text-gray-500 mt-0.5">
            Optional. Text files listing more paths to back up, one per line, passed to restic as{" "}
            <code className="text-gray-400">--files-from</code>. Lines starting with{" "}
            <code className="text-gray-400">#</code> are comments and glob patterns are expanded. Use
            absolute paths inside the file. restic reads it at backup time, so edits apply to the next
            run, and a missing file fails the backup.
          </p>
        </div>
        <div className="flex-shrink-0 whitespace-nowrap">
          <Button variant="secondary" size="sm" onClick={add}>+ File</Button>
        </div>
      </div>

      {filesFrom.length > 0 && (
        <ul className="space-y-1.5 mt-3">
          {filesFrom.map((p) => (
            <li key={p} className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2">
              <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
              <button
                type="button"
                onClick={() => update((prev) => ({ filesFrom: prev.filesFrom.filter((x) => x !== p) }))}
                className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                aria-label={`Remove ${p}`}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      {filesFrom.length > 0 && effectiveTags(form).length === 0 && (
        <p className="text-xs text-amber-400 mt-3">
          Plans that use path-list files need at least one tag (on the Tags tab), since their
          snapshots' paths come from the list files and can't be matched any other way.
        </p>
      )}
    </div>
  );
}
