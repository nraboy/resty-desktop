import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { v4 as uuidv4 } from "uuid";
import { open } from "@tauri-apps/plugin-dialog";
import {
  listBackupPlans,
  saveBackupPlan,
  removeBackupPlan,
  listRepos,
} from "../lib/invoke";
import type { BackupPlan, Repository } from "../lib/types";
import Button from "../components/Button";
import Input from "../components/Input";

export default function BackupPlanEditPage() {
  const { planId } = useParams<{ planId: string }>();
  const navigate = useNavigate();
  const isNew = planId === "new";

  const [repos, setRepos] = useState<Repository[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState("");

  const [name, setName] = useState("");
  const [repoId, setRepoId] = useState("");
  const [paths, setPaths] = useState<string[]>([]);
  const [tags, setTags] = useState<string[]>([]);
  const [tagInput, setTagInput] = useState("");
  const [excludeText, setExcludeText] = useState("");

  useEffect(() => {
    const init = async () => {
      setLoading(true);
      try {
        const [allRepos, allPlans] = await Promise.all([listRepos(), listBackupPlans()]);
        setRepos(allRepos);

        if (!isNew) {
          const plan = allPlans.find((p) => p.id === planId);
          if (plan) {
            setName(plan.name);
            const repoStillExists = allRepos.some((r) => r.id === plan.repoId);
            setRepoId(repoStillExists ? plan.repoId : "");
            if (!repoStillExists) {
              setError("The repository linked to this plan no longer exists. Please select a new one.");
            }
            setPaths(plan.paths);
            setTags(plan.tags);
            setExcludeText(plan.excludes.join("\n"));
          } else {
            setError("Backup plan not found.");
          }
        } else if (allRepos.length > 0) {
          setRepoId(allRepos[0].id);
        }
      } catch (err: any) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    };
    init();
  }, [planId, isNew]);

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

  const handleSave = async () => {
    if (!name.trim()) { setError("Plan name is required."); return; }
    if (!repoId) { setError("Select a target repository."); return; }
    if (paths.length === 0) { setError("Add at least one source path."); return; }

    setSaving(true);
    setError("");
    try {
      const plan: BackupPlan = {
        id: isNew ? uuidv4() : planId!,
        name: name.trim(),
        repoId,
        paths,
        tags,
        excludes: excludeText
          .split("\n")
          .map((l) => l.trim())
          .filter((l) => l && !l.startsWith("#")),
      };
      await saveBackupPlan(plan);
      navigate("/backup-plans");
    } catch (err: any) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (isNew || !planId) return;
    setDeleting(true);
    try {
      await removeBackupPlan(planId);
      navigate("/backup-plans");
    } catch (err: any) {
      setError(String(err));
      setDeleting(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-gray-500 text-sm">Loading…</p>
      </div>
    );
  }

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">
            {isNew ? "New Backup Plan" : "Edit Backup Plan"}
          </h1>
          <p className="text-sm text-gray-500 mt-0.5">
            Define what to back up and which repository to use.
          </p>
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      {/* Name */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
        <Input
          label="Plan Name"
          placeholder="e.g. Daily Documents Backup"
          value={name}
          onChange={(e) => setName(e.target.value)}
        />
      </div>

      {/* Target repository */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
        <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
          Target Repository
        </label>
        {repos.length === 0 ? (
          <p className="text-sm text-gray-500">
            No repositories configured.{" "}
            <button
              className="text-blue-400 hover:underline"
              onClick={() => navigate("/")}
            >
              Add one first.
            </button>
          </p>
        ) : (
          <select
            value={repoId}
            onChange={(e) => { setRepoId(e.target.value); setError(""); }}
            className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-sm text-gray-200 focus:outline-none focus:border-blue-500"
          >
            <option value="" disabled>Select a repository…</option>
            {repos.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name} — {r.path}
              </option>
            ))}
          </select>
        )}
      </div>

      {/* Source paths */}
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
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
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
                <button
                  onClick={() => removeTag(t)}
                  className="text-blue-400 hover:text-white transition-colors"
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Exclude patterns */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Exclude Patterns (optional)</h2>
        <p className="text-xs text-gray-500 mb-3">
          One pattern per line — same syntax as .gitignore. Lines starting with{" "}
          <code className="text-gray-400">#</code> are comments.
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

      <div className="flex items-center justify-between">
        <div className="flex gap-3">
          <Button onClick={handleSave} loading={saving}>
            {isNew ? "Create Plan" : "Save Changes"}
          </Button>
          <Button variant="secondary" onClick={() => navigate("/backup-plans")} disabled={saving}>
            Cancel
          </Button>
        </div>

        {!isNew && (
          <Button variant="danger" loading={deleting} onClick={handleDelete}>
            Delete Plan
          </Button>
        )}
      </div>
    </div>
  );
}
