import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { runBackup } from "../lib/invoke";
import { useAppStore } from "../store/appStore";
import Button from "../components/Button";
import Input from "../components/Input";
import EmptyState from "../components/EmptyState";

export default function BackupPage() {
  const navigate = useNavigate();
  const { activeRepo } = useAppStore();
  const [paths, setPaths] = useState<string[]>([]);
  const [tags, setTags] = useState<string[]>([]);
  const [tagInput, setTagInput] = useState("");
  const [excludeText, setExcludeText] = useState("");
  const [running, setRunning] = useState(false);
  const [output, setOutput] = useState("");
  const [error, setError] = useState("");
  const [done, setDone] = useState(false);

  const pickFolder = async () => {
    const selected = await open({ directory: true, multiple: true });
    if (!selected) return;
    const arr = Array.isArray(selected) ? selected : [selected];
    setPaths((prev) => [...new Set([...prev, ...arr])]);
  };

  const pickFile = async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const arr = Array.isArray(selected) ? selected : [selected];
    setPaths((prev) => [...new Set([...prev, ...arr])]);
  };

  const removePath = (p: string) => setPaths((prev) => prev.filter((x) => x !== p));

  const addTag = () => {
    const t = tagInput.trim();
    if (t && !tags.includes(t)) {
      setTags((prev) => [...prev, t]);
      setTagInput("");
    }
  };

  const removeTag = (t: string) => setTags((prev) => prev.filter((x) => x !== t));

  const startBackup = async () => {
    if (!activeRepo || paths.length === 0) return;
    setRunning(true);
    setError("");
    setOutput("");
    setDone(false);
    try {
      const excludes = excludeText.split("\n").map((l) => l.trim()).filter((l) => l && !l.startsWith("#"));
      const result = await runBackup(activeRepo, paths, tags, excludes);
      setOutput(result);
      setDone(true);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setRunning(false);
    }
  };

  if (!activeRepo) {
    return (
      <EmptyState
        title="No repository selected"
        description="Select a repository before running a backup."
        action={
          <Button variant="secondary" onClick={() => navigate("/")}>
            Go to Repositories
          </Button>
        }
      />
    );
  }

  return (
    <div className="p-6 max-w-2xl">
      <div className="mb-6">
        <h1 className="text-xl font-semibold text-gray-100">New Backup</h1>
        <p className="text-sm text-gray-500 mt-0.5">
          Backing up to <span className="font-medium text-gray-300">{activeRepo.name}</span>
        </p>
      </div>

      {/* Path selection */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-gray-300">Source Paths</h2>
          <div className="flex gap-2">
            <Button variant="ghost" size="sm" onClick={pickFile}>+ Files</Button>
            <Button variant="secondary" size="sm" onClick={pickFolder}>+ Folder</Button>
          </div>
        </div>

        {paths.length === 0 ? (
          <p className="text-sm text-gray-600 text-center py-4">
            No paths selected. Add a file or folder to back up.
          </p>
        ) : (
          <ul className="space-y-1.5">
            {paths.map((p) => (
              <li
                key={p}
                className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2"
              >
                <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                <button
                  onClick={() => removePath(p)}
                  className="text-gray-500 hover:text-red-400 transition-colors ml-2 flex-shrink-0"
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* Tags */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <h2 className="text-sm font-medium text-gray-300 mb-3">Tags (optional)</h2>
        <div className="flex gap-2 mb-2">
          <Input
            placeholder="Add tag…"
            value={tagInput}
            onChange={(e) => setTagInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && addTag()}
            className="flex-1"
          />
          <Button variant="secondary" size="sm" onClick={addTag}>Add</Button>
        </div>
        {tags.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mt-2">
            {tags.map((t) => (
              <span
                key={t}
                className="inline-flex items-center gap-1 text-xs bg-blue-900/40 text-blue-300 border border-blue-700/50 px-2 py-0.5 rounded-full"
              >
                {t}
                <button onClick={() => removeTag(t)} className="text-blue-400 hover:text-white transition-colors">×</button>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Excludes */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Exclude Patterns (optional)</h2>
        <p className="text-xs text-gray-500 mb-3">
          One pattern per line — same syntax as .gitignore. Lines starting with <code className="text-gray-400">#</code> are comments.
        </p>
        <textarea
          value={excludeText}
          onChange={(e) => setExcludeText(e.target.value)}
          placeholder={"*.log\nnode_modules/\n# ignore temp files\n*.tmp"}
          rows={6}
          className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500 resize-y"
          spellCheck={false}
        />
      </div>

      {/* Error */}
      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300 font-mono whitespace-pre-wrap">
          {error}
        </div>
      )}

      {/* Output */}
      {output && (
        <div className="mb-4 p-3 bg-gray-900 border border-gray-700 rounded-lg text-xs text-gray-300 font-mono whitespace-pre-wrap max-h-48 overflow-y-auto">
          {output}
        </div>
      )}

      {done && (
        <div className="mb-4 p-3 bg-green-900/30 border border-green-700 rounded-lg text-sm text-green-300">
          Backup completed successfully.
        </div>
      )}

      <div className="flex gap-3">
        <Button
          onClick={startBackup}
          loading={running}
          disabled={paths.length === 0}
          size="lg"
        >
          {running ? "Running backup…" : "Start Backup"}
        </Button>
        {done && (
          <Button variant="secondary" size="lg" onClick={() => navigate("/snapshots")}>
            View Snapshots
          </Button>
        )}
      </div>
    </div>
  );
}
