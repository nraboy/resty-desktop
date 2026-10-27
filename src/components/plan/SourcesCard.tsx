import { open } from "@tauri-apps/plugin-dialog";
import Button from "../Button";
import { CARD, type FormProps } from "./types";

/**
 * The plain list of source paths, with file/folder pickers. The file tree (PlanTreePreview) is how
 * sources are normally chosen; this is the fallback for a platform where the tree can't run.
 */
export default function SourcesCard({ form, update }: FormProps) {
  const pick = async (opts: { directory?: boolean }) => {
    const selected = await open({ ...opts, multiple: true });
    if (!selected) return;
    const picked = Array.isArray(selected) ? selected : [selected];
    update((p) => ({ paths: [...new Set([...p.paths, ...picked])] }));
  };

  const { paths } = form;
  return (
    <div className={CARD}>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-medium text-gray-300">Sources</h2>
        <div className="flex gap-2">
          <Button variant="ghost" size="sm" onClick={() => pick({})}>+ Files</Button>
          <Button variant="secondary" size="sm" onClick={() => pick({ directory: true })}>+ Folder</Button>
        </div>
      </div>

      {paths.length === 0 ? (
        <p className="text-sm text-gray-500 text-center py-4">No paths selected. Add a file or folder to back up.</p>
      ) : (
        <ul className="space-y-1.5">
          {paths.map((p) => (
            <li key={p} className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2">
              <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
              <button
                type="button"
                onClick={() => update((prev) => ({ paths: prev.paths.filter((x) => x !== p) }))}
                className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                aria-label={`Remove ${p}`}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
