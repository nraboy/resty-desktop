import { useState } from "react";
import Button from "../Button";
import Input from "../Input";
import { CARD, type FormProps } from "./types";

/** `--exclude-if-present` marker files, and `--exclude-caches`. */
export default function MarkersCard({ form, update }: FormProps) {
  const [input, setInput] = useState("");
  const { excludeIfPresent, excludeCaches } = form;

  const add = () => {
    const v = input.trim();
    if (v && !excludeIfPresent.includes(v)) {
      update((p) => ({ excludeIfPresent: [...p.excludeIfPresent, v] }));
      setInput("");
    }
  };

  const v = input.trim();
  return (
    <div className={CARD}>
      <h2 className="text-sm font-medium text-gray-300 mb-1">Exclude If Present (optional)</h2>
      <p className="text-xs text-gray-500 mb-3">
        If a directory contains a file with one of these names, its contents are skipped —
        the marker file itself is still backed up. Useful for marking temporary/scratch data
        (e.g. a <code className="text-gray-400">.nobackup</code> file) instead of listing every
        such directory as its own exclusion above. Also accepts restic's{" "}
        <code className="text-gray-400">name:header</code> syntax to match only when the
        marker file starts with given content.
      </p>

      <div className="flex gap-2 mb-2">
        <Input
          placeholder=".nobackup"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), add())}
          className="flex-1"
        />
        <Button variant="secondary" size="sm" onClick={add}>
          Add
        </Button>
      </div>

      {v && (v.includes("/") || v.includes("\\")) && (
        <p className="text-xs text-amber-400 mb-2">
          Marker files are matched by name only — remove the path (e.g. use{" "}
          <code className="text-amber-400">.nobackup</code> instead of{" "}
          <code className="text-amber-400">data/.nobackup</code>).
        </p>
      )}
      {v && !(v.includes("/") || v.includes("\\")) && v.startsWith("#") && (
        <p className="text-xs text-amber-400 mb-2">
          Filenames starting with <code className="text-amber-400">#</code> are ignored,
          same as a comment in Exclude Patterns.
        </p>
      )}

      {excludeIfPresent.length > 0 && (
        <ul className="space-y-1.5 mt-2">
          {excludeIfPresent.map((p) => (
            <li key={p} className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2">
              <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
              <button
                onClick={() => update((prev) => ({ excludeIfPresent: prev.excludeIfPresent.filter((x) => x !== p) }))}
                className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      {excludeIfPresent.length === 0 && (
        <p className="text-sm text-gray-500 text-center py-3">
          No marker files. Directories containing a file added above will have their contents skipped.
        </p>
      )}

      <label className="flex items-center gap-2 mt-3 pt-3 border-t border-gray-800">
        <input
          type="checkbox"
          checked={excludeCaches}
          onChange={(e) => update({ excludeCaches: e.target.checked })}
          className="rounded bg-gray-700 border-gray-600"
        />
        <span className="text-sm text-gray-300">
          Also exclude cache directories (<code className="text-gray-400">CACHEDIR.TAG</code>)
        </span>
      </label>
    </div>
  );
}
