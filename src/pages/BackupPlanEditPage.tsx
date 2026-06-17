import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
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

type ExcludeMode = "simple" | "expert";

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

function needsFullDiskAccess(p: string): boolean {
  return (
    /\/Library(\/|$)/.test(p) ||
    p === "/System" || p.startsWith("/System/") ||
    p === "/private" || p.startsWith("/private/") ||
    p === "/var" || p.startsWith("/var/")
  );
}

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
  const [excludeMode, setExcludeMode] = useState<ExcludeMode>("simple");
  const [excludeItems, setExcludeItems] = useState<string[]>([]);
  const [excludeInput, setExcludeInput] = useState("");
  const [excludeText, setExcludeText] = useState("");
  const [keepLast, setKeepLast] = useState("");
  const [keepDaily, setKeepDaily] = useState("");
  const [keepWeekly, setKeepWeekly] = useState("");
  const [keepMonthly, setKeepMonthly] = useState("");
  const [keepYearly, setKeepYearly] = useState("");
  const [limitUpload, setLimitUpload] = useState("");
  const [limitDownload, setLimitDownload] = useState("");

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
            setExcludeItems(plan.excludes);
            setExcludeText(plan.excludes.join("\n"));
            setKeepLast(plan.retention?.keepLast?.toString() ?? "");
            setKeepDaily(plan.retention?.keepDaily?.toString() ?? "");
            setKeepWeekly(plan.retention?.keepWeekly?.toString() ?? "");
            setKeepMonthly(plan.retention?.keepMonthly?.toString() ?? "");
            setKeepYearly(plan.retention?.keepYearly?.toString() ?? "");
            setLimitUpload(plan.limitUpload?.toString() ?? "");
            setLimitDownload(plan.limitDownload?.toString() ?? "");
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

  const pickFolder = useCallback(async () => {
    const selected = await open({ directory: true, multiple: true });
    if (!selected) return;
    const arr = Array.isArray(selected) ? selected : [selected];
    setPaths((prev) => [...new Set([...prev, ...arr])]);
  }, []);

  const pickFile = useCallback(async () => {
    const selected = await open({ multiple: true });
    if (!selected) return;
    const arr = Array.isArray(selected) ? selected : [selected];
    setPaths((prev) => [...new Set([...prev, ...arr])]);
  }, []);

  const removePath = useCallback((p: string) => setPaths((prev) => prev.filter((x) => x !== p)), []);

  const addTag = useCallback(() => {
    const t = tagInput.trim();
    if (t && !tags.includes(t)) {
      setTags((prev) => [...prev, t]);
      setTagInput("");
    }
  }, [tagInput, tags]);

  const removeTag = useCallback((t: string) => setTags((prev) => prev.filter((x) => x !== t)), []);

  const addExclude = useCallback(() => {
    const v = excludeInput.trim();
    if (v && !excludeItems.includes(v)) {
      setExcludeItems((prev) => [...prev, v]);
      setExcludeInput("");
    }
  }, [excludeInput, excludeItems]);

  const removeExclude = useCallback(
    (p: string) => setExcludeItems((prev) => prev.filter((x) => x !== p)),
    [],
  );

  const switchExcludeMode = useCallback(
    (mode: ExcludeMode) => {
      if (mode === excludeMode) return;
      if (mode === "expert") {
        setExcludeText(excludeItems.join("\n"));
      } else {
        const parsed = excludeText
          .split("\n")
          .map((l) => l.trim())
          .filter((l) => l && !l.startsWith("#"));
        setExcludeItems([...new Set(parsed)]);
      }
      setExcludeMode(mode);
    },
    [excludeMode, excludeItems, excludeText],
  );

  const toggleSuggestion = useCallback(
    (patterns: string[]) => {
      const allPresent = patterns.every((p) => excludeItems.includes(p));
      if (allPresent) {
        setExcludeItems((prev) => prev.filter((p) => !patterns.includes(p)));
      } else {
        setExcludeItems((prev) => [...new Set([...prev, ...patterns])]);
      }
    },
    [excludeItems],
  );

  const handleSave = async () => {
    if (!name.trim()) { setError("Plan name is required."); return; }
    if (!repoId) { setError("Select a target repository."); return; }
    if (paths.length === 0) { setError("Add at least one source path."); return; }

    setSaving(true);
    setError("");
    try {
      const toNum = (s: string) => s.trim() === "" ? undefined : parseInt(s, 10);
      const retentionFields = [keepLast, keepDaily, keepWeekly, keepMonthly, keepYearly];
      const retention = retentionFields.some((s) => s.trim() !== "")
        ? {
            keepLast: toNum(keepLast),
            keepDaily: toNum(keepDaily),
            keepWeekly: toNum(keepWeekly),
            keepMonthly: toNum(keepMonthly),
            keepYearly: toNum(keepYearly),
          }
        : undefined;
      const excludes =
        excludeMode === "expert"
          ? excludeText
              .split("\n")
              .map((l) => l.trim())
              .filter((l) => l && !l.startsWith("#"))
          : excludeItems;
      const plan: BackupPlan = {
        id: isNew ? crypto.randomUUID() : planId!,
        name: name.trim(),
        repoId,
        paths,
        tags,
        excludes,
        retention,
        limitUpload: toNum(limitUpload),
        limitDownload: toNum(limitDownload),
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
          <div className="relative">
            <select
              value={repoId}
              onChange={(e) => { setRepoId(e.target.value); setError(""); }}
              className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-md px-3 py-2 text-sm text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500 pr-8"
            >
              <option value="" disabled>Select a repository…</option>
              {repos.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name} — {r.path}
                </option>
              ))}
            </select>
            <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">
              ▾
            </div>
          </div>
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
                  className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                >
                  ×
                </button>
              </li>
            ))}
          </ul>
        )}

        {paths.some(needsFullDiskAccess) && (
          <div className="mt-3 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg text-xs text-amber-300">
            <span className="font-medium">Full Disk Access may be required.</span>{" "}
            One or more paths (e.g. <code className="text-amber-300">~/Library</code>, system directories) are protected by macOS and cannot be read without Full Disk Access. Go to{" "}
            <span className="font-medium">System Settings → Privacy &amp; Security → Full Disk Access</span>{" "}
            and add Resty Desktop to avoid permission errors.
          </div>
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
                  className="text-blue-400 hover:text-gray-50 transition-colors"
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
      </div>

      {/* Exclude patterns */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
        <div className="flex items-center justify-between mb-3">
          <h2 className="text-sm font-medium text-gray-300">Exclude Patterns (optional)</h2>
          <div className="flex rounded-lg overflow-hidden border border-gray-700">
            <button
              type="button"
              onClick={() => switchExcludeMode("simple")}
              className={`px-3 py-1 text-xs font-medium transition-colors ${excludeMode === "simple" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
            >
              Simple
            </button>
            <button
              type="button"
              onClick={() => switchExcludeMode("expert")}
              className={`px-3 py-1 text-xs font-medium transition-colors ${excludeMode === "expert" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
            >
              Expert
            </button>
          </div>
        </div>

        {excludeMode === "simple" ? (
          <>
            {/* Suggestions */}
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
                      {active && <span className="text-blue-400">✓</span>}
                      {s.label}
                    </button>
                  );
                })}
              </div>
            </div>

            {/* Manual add */}
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
                {excludeItems.map((p) => (
                  <li
                    key={p}
                    className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2"
                  >
                    <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                    <button
                      onClick={() => removeExclude(p)}
                      className="text-gray-500 hover:text-red-300 transition-colors ml-2 flex-shrink-0"
                    >
                      ×
                    </button>
                  </li>
                ))}
              </ul>
            )}

            {excludeItems.length === 0 && (
              <p className="text-sm text-gray-600 text-center py-3">
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
              onChange={(e) => setExcludeText(e.target.value)}
              placeholder={"*.log\nnode_modules/\n# ignore temp files\n*.tmp"}
              rows={6}
              className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500 resize-y"
              spellCheck={false}
            />
          </>
        )}
      </div>

      {/* Retention policy */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Retention Policy (optional)</h2>
        <p className="text-xs text-gray-500 mb-4">
          After each backup, old snapshots will be pruned. Leave all fields blank to skip pruning.
        </p>
        <div className="grid grid-cols-2 gap-x-6 gap-y-3">
          {[
            { label: "Keep last", unit: "snapshots", value: keepLast, set: setKeepLast },
            { label: "Keep daily", unit: "days", value: keepDaily, set: setKeepDaily },
            { label: "Keep weekly", unit: "weeks", value: keepWeekly, set: setKeepWeekly },
            { label: "Keep monthly", unit: "months", value: keepMonthly, set: setKeepMonthly },
            { label: "Keep yearly", unit: "years", value: keepYearly, set: setKeepYearly },
          ].map(({ label, unit, value, set }) => (
            <div key={label} className="flex items-center gap-3">
              <label className="text-xs text-gray-400 w-28 flex-shrink-0">{label}</label>
              <input
                type="number"
                min="0"
                value={value}
                onChange={(e) => set(e.target.value)}
                placeholder="—"
                className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500"
              />
              <span className="text-xs text-gray-500">{unit}</span>
            </div>
          ))}
        </div>
      </div>

      {/* Bandwidth limits */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Bandwidth Limits (optional)</h2>
        <p className="text-xs text-gray-500 mb-4">
          Limits are in KiB/s. Leave blank for unlimited. These settings only affect remote repositories (S3, SFTP, etc.) — they have no effect on local repos.
        </p>
        <div className="grid grid-cols-2 gap-x-6 gap-y-3">
          {[
            { label: "Upload limit", value: limitUpload, set: setLimitUpload },
            { label: "Download limit", value: limitDownload, set: setLimitDownload },
          ].map(({ label, value, set }) => (
            <div key={label} className="flex items-center gap-3">
              <label className="text-xs text-gray-400 w-28 flex-shrink-0">{label}</label>
              <input
                type="number"
                min="0"
                value={value}
                onChange={(e) => set(e.target.value)}
                placeholder="—"
                className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-600 focus:outline-none focus:border-blue-500"
              />
              <span className="text-xs text-gray-500">KiB/s</span>
            </div>
          ))}
        </div>
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
