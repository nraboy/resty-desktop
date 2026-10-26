import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { ExcludeMode } from "../../lib/planForm";
import Button from "../Button";
import { CheckIcon } from "../icons";
import Input from "../Input";
import SegmentedToggle from "../SegmentedToggle";
import { CARD, type FormProps } from "./types";

const EXCLUDE_SUGGESTIONS = [
  {
    id: "dev",
    label: "Development assets",
    description: "node_modules, build output, caches, lockfiles",
    patterns: [
      "node_modules/",
      ".git/",
      "__pycache__/",
      "*.pyc",
      ".venv/",
      "venv/",
      "target/",
      "vendor/",
      "build/",
      "dist/",
      ".next/",
      ".nuxt/",
      ".gradle/",
      ".cargo/registry/",
    ],
  },
  {
    id: "system",
    label: "System files",
    description: ".DS_Store, Thumbs.db, desktop.ini",
    patterns: [".DS_Store", "Thumbs.db", "desktop.ini", "ehthumbs.db"],
  },
  {
    id: "logs",
    label: "Log files",
    description: "*.log and rotated log variants",
    patterns: ["*.log", "*.log.*", "logs/"],
  },
  {
    id: "temp",
    label: "Temporary files",
    description: "*.tmp, swap files, backups",
    patterns: ["*.tmp", "*.temp", "*.swp", "*.bak", "~*"],
  },
];

/** Exclude patterns (Simple list with presets / Expert text) and exclude pattern files. */
export default function ExcludeCard({ form, update }: FormProps) {
  const [excludeInput, setExcludeInput] = useState("");
  const { excludeItems, excludeMode, excludeText, excludeFiles } = form;

  const addExclude = () => {
    const v = excludeInput.trim();
    if (v && !excludeItems.includes(v)) {
      update((p) => ({ excludeItems: [...p.excludeItems, v] }));
      setExcludeInput("");
    }
  };

  const switchMode = (mode: ExcludeMode) => {
    if (mode === excludeMode) return;
    if (mode === "expert") {
      update({ excludeText: excludeItems.join("\n"), excludeMode: mode });
    } else {
      // Leaving expert: fold the textarea back into the list so `excludeItems` is the single
      // source of truth whenever expert isn't active.
      const parsed = excludeText
        .split("\n")
        .map((l) => l.trim())
        .filter((l) => l && !l.startsWith("#"));
      update({ excludeItems: [...new Set(parsed)], excludeMode: mode });
    }
  };

  const toggleSuggestion = (patterns: string[]) => {
    const allPresent = patterns.every((p) => excludeItems.includes(p));
    if (allPresent) {
      update((prev) => ({ excludeItems: prev.excludeItems.filter((p) => !patterns.includes(p)) }));
    } else {
      update((prev) => ({ excludeItems: [...new Set([...prev.excludeItems, ...patterns])] }));
    }
  };

  const pickExcludeFiles = async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const arr = Array.isArray(selected) ? selected : [selected];
    update((prev) => {
      const known = new Set(prev.excludeFiles.map((f) => f.path));
      const added = arr.filter((p) => !known.has(p)).map((path) => ({ path, ignoreCase: false }));
      return { excludeFiles: [...prev.excludeFiles, ...added] };
    });
  };

  return (
    <div className={CARD}>
      <div className="flex items-center justify-between mb-3">
        <h2 className="text-sm font-medium text-gray-300">Exclude Patterns (optional)</h2>
        <SegmentedToggle<ExcludeMode>
          value={excludeMode}
          onChange={switchMode}
          options={[
            { value: "simple", label: "Simple" },
            { value: "expert", label: "Expert" },
          ]}
        />
      </div>

      {excludeMode === "simple" ? (
        <>
          <div className="mb-3">
            <p className="text-xs text-gray-500 mb-2">Quick suggestions</p>
            <div className="flex flex-wrap gap-2">
              {EXCLUDE_SUGGESTIONS.map((s) => {
                const active = s.patterns.every((p) => excludeItems.includes(p));
                return (
                  <button
                    key={s.id}
                    type="button"
                    title={s.description}
                    onClick={() => toggleSuggestion(s.patterns)}
                    className={`inline-flex items-center gap-1.5 text-xs px-2.5 py-1 rounded-full border transition-colors ${
                      active
                        ? "bg-blue-900/50 border-blue-600 text-blue-300"
                        : "bg-gray-800 border-gray-700 text-gray-400 hover:border-gray-500 hover:text-gray-300"
                    }`}
                  >
                    {active && (
                      <span className="text-blue-400 flex-shrink-0">
                        <CheckIcon className="w-3.5 h-3.5" />
                      </span>
                    )}
                    {s.label}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="flex gap-2 mb-2">
            <Input
              placeholder="e.g. *.log or node_modules/"
              value={excludeInput}
              onChange={(e) => setExcludeInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), addExclude())}
              className="flex-1"
            />
            <Button variant="secondary" size="sm" onClick={addExclude}>
              Add
            </Button>
          </div>

          {excludeItems.length > 0 && (
            <ul className="space-y-1.5 mt-2">
              {excludeItems.map((p, i) => (
                // Keyed by position: patterns can legitimately repeat (a later `!x` after an
                // earlier one), and removal is by position too, since order is meaning here.
                <li
                  key={`${i}:${p}`}
                  className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2"
                >
                  <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                  <button
                    onClick={() => update((prev) => ({ excludeItems: prev.excludeItems.filter((_, j) => j !== i) }))}
                    className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                  >
                    ×
                  </button>
                </li>
              ))}
            </ul>
          )}

          {excludeItems.length === 0 && (
            <p className="text-sm text-gray-500 text-center py-3">
              No exclusions. Files and folders added above will be skipped during backup.
            </p>
          )}
        </>
      ) : (
        <>
          <p className="text-xs text-gray-500 mb-3">
            One pattern per line — same syntax as .gitignore. Lines starting with{" "}
            <code className="text-gray-400">#</code> are comments.
          </p>
          <textarea
            value={excludeText}
            onChange={(e) => update({ excludeText: e.target.value })}
            placeholder={"*.log\nnode_modules/\n# ignore temp files\n*.tmp"}
            rows={6}
            className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500 resize-y"
            spellCheck={false}
          />
        </>
      )}

      <div className="mt-4 pt-4 border-t border-gray-800">
        <div className="flex items-center justify-between mb-1">
          <h3 className="text-sm font-medium text-gray-300">Exclude Pattern Files</h3>
          <Button variant="secondary" size="sm" onClick={pickExcludeFiles}>+ File</Button>
        </div>
        <p className="text-xs text-gray-500 mb-3">
          Text files of exclude patterns, one per line, with the same syntax as Expert mode
          (passed as <code className="text-gray-400">--exclude-file</code>). Applied together
          with the patterns above. Tick <span className="text-gray-400">Ignore case</span> to
          match regardless of capitalization (
          <code className="text-gray-400">--iexclude-file</code>).
        </p>

        {excludeFiles.length === 0 ? (
          <p className="text-sm text-gray-500 text-center py-3">No exclude pattern files.</p>
        ) : (
          <ul className="space-y-1.5">
            {excludeFiles.map((f) => (
              <li
                key={f.path}
                className="flex items-center justify-between gap-3 bg-gray-800 rounded-lg px-3 py-2"
              >
                <span className="text-xs font-mono text-gray-300 truncate flex-1">{f.path}</span>
                <label className="flex items-center gap-1.5 text-xs text-gray-400 flex-shrink-0 whitespace-nowrap">
                  <input
                    type="checkbox"
                    checked={f.ignoreCase}
                    onChange={() =>
                      update((prev) => ({
                        excludeFiles: prev.excludeFiles.map((x) => (x.path === f.path ? { ...x, ignoreCase: !x.ignoreCase } : x)),
                      }))
                    }
                    className="rounded bg-gray-700 border-gray-600"
                  />
                  Ignore case
                </label>
                <button
                  onClick={() => update((prev) => ({ excludeFiles: prev.excludeFiles.filter((x) => x.path !== f.path) }))}
                  className="text-gray-500 hover:text-red-300 transition-colors flex-shrink-0"
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
