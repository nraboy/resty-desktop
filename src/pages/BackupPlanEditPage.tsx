import { useCallback, useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import {
  checkFullDiskAccess,
  listBackupPlans,
  openFullDiskAccessSettings,
  saveBackupPlan,
  removeBackupPlan,
  listRepos,
  previewWebhook,
  testWebhook,
} from "../lib/invoke";
import type { FullDiskAccessStatus } from "../lib/invoke";
import type { BackupPlan, PlanWebhook, Repository, WebhookPreview, WebhookProvider } from "../lib/types";
import { needsFullDiskAccess } from "../lib/utils";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import { ChevronDownIcon, CheckIcon, PencilIcon, XIcon } from "../components/icons";

type ExcludeMode = "simple" | "expert";

/** Pre-filled body for a new custom-provider webhook — pinned by a Rust test
 *  (webhook.rs's DEFAULT_TEMPLATE) so the two can't drift apart. */
const DEFAULT_WEBHOOK_TEMPLATE =
  '{"event": "{eventName}", "plan": "{planName}", "repo": "{repoName}", "durationSeconds": {durationSeconds}}';

/** Placeholders a custom template may reference — rendered by the backend, listed
 *  here only as the editing hint (interpolation itself is Rust-side). */
const WEBHOOK_PLACEHOLDERS = [
  "{eventName}",
  "{repoName}",
  "{planName}",
  "{startedAt}",
  "{durationSeconds}",
  "{filesNew}",
  "{filesChanged}",
  "{bytesAdded}",
  "{snapshotId}",
  "{errorMessage}",
];

const WEBHOOK_PROVIDER_LABELS: Record<WebhookProvider, string> = {
  generic: "Generic JSON",
  discord: "Discord",
  slack: "Slack",
  teams: "Teams",
  custom: "Custom JSON",
};

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


export default function BackupPlanEditPage() {
  const { planId } = useParams<{ planId: string }>();
  const navigate = useNavigate();
  const isNew = planId === "new";

  const [repos, setRepos] = useState<Repository[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
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
  const [excludeIfPresent, setExcludeIfPresent] = useState<string[]>([]);
  const [excludeIfPresentInput, setExcludeIfPresentInput] = useState("");
  const [excludeCaches, setExcludeCaches] = useState(false);
  const [keepLast, setKeepLast] = useState("");
  const [keepDaily, setKeepDaily] = useState("");
  const [keepWeekly, setKeepWeekly] = useState("");
  const [keepMonthly, setKeepMonthly] = useState("");
  const [keepYearly, setKeepYearly] = useState("");
  const [limitUpload, setLimitUpload] = useState("");
  const [limitDownload, setLimitDownload] = useState("");
  const [webhooks, setWebhooks] = useState<PlanWebhook[]>([]);
  // Add/Edit Webhook modal — all webhook configuration happens there; the card
  // itself is a read-only list (URL, provider, trigger summary, edit/delete).
  const [webhookModalOpen, setWebhookModalOpen] = useState(false);
  const [editingWebhookId, setEditingWebhookId] = useState<string | null>(null);
  const [draftUrl, setDraftUrl] = useState("");
  const [draftProvider, setDraftProvider] = useState<WebhookProvider>("generic");
  const [draftStages, setDraftStages] = useState({ started: false, completed: true, failed: true });
  const [draftTemplate, setDraftTemplate] = useState(DEFAULT_WEBHOOK_TEMPLATE);
  // One message slot for the whole modal — Save validation failures and Send Test
  // results share it, so they render in one style and a new message always
  // replaces (never stacks on) the previous one.
  const [webhookNotice, setWebhookNotice] = useState<{ ok: boolean; message: string } | null>(null);
  const [testingWebhook, setTestingWebhook] = useState(false);
  const [webhookPreview, setWebhookPreview] = useState<WebhookPreview | null>(null);
  const [webhookPreviewLoading, setWebhookPreviewLoading] = useState(false);
  const [webhookPreviewError, setWebhookPreviewError] = useState("");
  const previewSeqRef = useRef(0);
  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);

  useEffect(() => {
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
  }, []);

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
            setExcludeIfPresent(plan.excludeIfPresent);
            setExcludeCaches(plan.excludeCaches);
            setKeepLast(plan.retention?.keepLast?.toString() ?? "");
            setKeepDaily(plan.retention?.keepDaily?.toString() ?? "");
            setKeepWeekly(plan.retention?.keepWeekly?.toString() ?? "");
            setKeepMonthly(plan.retention?.keepMonthly?.toString() ?? "");
            setKeepYearly(plan.retention?.keepYearly?.toString() ?? "");
            setLimitUpload(plan.limitUpload?.toString() ?? "");
            setLimitDownload(plan.limitDownload?.toString() ?? "");
            setWebhooks(plan.webhooks ?? []);
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

  const addExcludeIfPresent = useCallback(() => {
    const v = excludeIfPresentInput.trim();
    if (v && !excludeIfPresent.includes(v)) {
      setExcludeIfPresent((prev) => [...prev, v]);
      setExcludeIfPresentInput("");
    }
  }, [excludeIfPresentInput, excludeIfPresent]);

  const removeExcludeIfPresent = useCallback(
    (p: string) => setExcludeIfPresent((prev) => prev.filter((x) => x !== p)),
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

  const openAddWebhookModal = useCallback(() => {
    setEditingWebhookId(null);
    setDraftUrl("");
    setDraftProvider("generic");
    setDraftStages({ started: false, completed: true, failed: true });
    setDraftTemplate(DEFAULT_WEBHOOK_TEMPLATE);
    setWebhookNotice(null);
    setWebhookModalOpen(true);
  }, []);

  const openEditWebhookModal = useCallback((w: PlanWebhook) => {
    setEditingWebhookId(w.id);
    setDraftUrl(w.url);
    setDraftProvider(w.provider);
    setDraftStages({ ...w.stages });
    // A legacy preset row has no template — switching it to Custom in the modal
    // should start from the working default, not an empty textarea.
    setDraftTemplate(w.template ?? DEFAULT_WEBHOOK_TEMPLATE);
    setWebhookNotice(null);
    setWebhookModalOpen(true);
  }, []);

  const closeWebhookModal = useCallback(() => {
    setWebhookModalOpen(false);
    setEditingWebhookId(null);
  }, []);

  const removeWebhook = useCallback((id: string) => {
    setWebhooks((prev) => prev.filter((w) => w.id !== id));
    if (editingWebhookId === id) closeWebhookModal();
  }, [editingWebhookId, closeWebhookModal]);

  const commitWebhook = useCallback(() => {
    const url = draftUrl.trim();
    if (!/^https?:\/\//.test(url)) {
      setWebhookNotice({ ok: false, message: "Webhook URL must start with http:// or https://." });
      return;
    }
    if (draftProvider === "custom" && !draftTemplate.trim()) {
      setWebhookNotice({ ok: false, message: "Custom JSON webhooks need a JSON body template." });
      return;
    }
    // The preview refetch is async — while it's in flight, webhookPreviewError still
    // holds the *previous* draft's verdict, so committing now could either pass a
    // just-broken template or block a just-fixed one. Wait for the fresh verdict.
    if (draftProvider === "custom" && webhookPreviewLoading) {
      setWebhookNotice({ ok: false, message: "Validating template — try saving again in a moment." });
      return;
    }
    // preview_webhook already holds the parse verdict for the current draft — a
    // custom template that doesn't render valid JSON can't be committed.
    if (draftProvider === "custom" && webhookPreviewError) {
      setWebhookNotice({ ok: false, message: webhookPreviewError });
      return;
    }
    if (editingWebhookId) {
      setWebhooks((prev) =>
        prev.map((w) =>
          w.id === editingWebhookId
            ? {
                ...w,
                url,
                provider: draftProvider,
                stages: { ...draftStages },
                template: draftProvider === "custom" ? draftTemplate : undefined,
              }
            : w,
        ),
      );
    } else {
      setWebhooks((prev) => [
        ...prev,
        {
          id: crypto.randomUUID(),
          url,
          provider: draftProvider,
          stages: { ...draftStages },
          ...(draftProvider === "custom" ? { template: draftTemplate } : {}),
        },
      ]);
    }
    closeWebhookModal();
  }, [draftUrl, draftProvider, draftStages, draftTemplate, editingWebhookId, webhookPreviewError, webhookPreviewLoading, closeWebhookModal]);

  const toggleDraftStage = useCallback((stage: "started" | "completed" | "failed") => {
    setDraftStages((prev) => ({ ...prev, [stage]: !prev[stage] }));
    setWebhookNotice(null);
  }, []);

  const sendTestWebhook = useCallback(async () => {
    setTestingWebhook(true);
    setWebhookNotice(null);
    try {
      await testWebhook(draftUrl.trim(), draftProvider, draftProvider === "custom" ? draftTemplate : undefined);
      setWebhookNotice({ ok: true, message: "Webhook delivered — check the target service." });
    } catch (err: any) {
      setWebhookNotice({ ok: false, message: String(err) });
    } finally {
      setTestingWebhook(false);
    }
  }, [draftUrl, draftProvider, draftTemplate]);

  // The modal's payload preview — always rendered while the modal is open, re-fetched
  // whenever the draft's provider/template changes so it always matches what would fire.
  const previewTemplate = draftProvider === "custom" ? draftTemplate : undefined;
  useEffect(() => {
    if (!webhookModalOpen) {
      setWebhookPreview(null);
      setWebhookPreviewError("");
      return;
    }
    const seq = ++previewSeqRef.current;
    setWebhookPreviewLoading(true);
    previewWebhook(draftProvider, previewTemplate)
      .then((p) => {
        if (seq !== previewSeqRef.current) return;
        setWebhookPreview(p);
        setWebhookPreviewError("");
      })
      .catch((err: any) => {
        if (seq !== previewSeqRef.current) return;
        setWebhookPreview(null);
        setWebhookPreviewError(String(err));
      })
      .finally(() => {
        if (seq === previewSeqRef.current) setWebhookPreviewLoading(false);
      });
  }, [webhookModalOpen, draftProvider, previewTemplate]);

  const handleSave = async () => {
    if (!name.trim()) { setError("Plan name is required."); return; }
    if (!repoId) { setError("Select a target repository."); return; }
    if (paths.length === 0) { setError("Add at least one source path."); return; }
    // The modal enforces this for every row it creates; this guard only catches rows
    // that arrived from outside the UI (a hand-edited import bundle).
    const badUrl = webhooks.find((w) => !/^https?:\/\//.test(w.url.trim()));
    if (badUrl) {
      setError(`Webhook "${badUrl.url}" must start with http:// or https://.`);
      return;
    }

    setSaving(true);
    setError("");
    try {
      const toNum = (s: string) => {
        if (s.trim() === "") return undefined;
        const n = parseInt(s, 10);
        return Number.isNaN(n) ? undefined : n;
      };
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
        excludeIfPresent,
        excludeCaches,
        retention,
        limitUpload: toNum(limitUpload),
        limitDownload: toNum(limitDownload),
        webhooks,
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
      setConfirmDelete(false);
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
              {repos
                // A backup plan can never target a read-only repo — except the plan's
                // already-selected repo, kept visible-but-disabled below so an existing
                // plan whose repo has since become read-only isn't silently dropped.
                .filter((r) => !r.readOnly || r.id === repoId)
                .map((r) => (
                  <option key={r.id} value={r.id} disabled={r.readOnly}>
                    {r.name} — {r.path}{r.readOnly ? " (read-only)" : ""}
                  </option>
                ))}
            </select>
            <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">
              <ChevronDownIcon className="w-4 h-4" />
            </div>
          </div>
        )}
        {repos.find((r) => r.id === repoId)?.readOnly && (
          <p className="text-sm text-amber-400 mt-2">
            This plan's repository is marked read-only — backups can't run until you either
            select a different repository or clear the read-only flag on Repositories.
          </p>
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
          <p className="text-sm text-gray-500 text-center py-4">
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

        {paths.some(needsFullDiskAccess) && (fdaStatus?.supported && !fdaStatus.granted) && (
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
              onChange={(e) => setExcludeText(e.target.value)}
              placeholder={"*.log\nnode_modules/\n# ignore temp files\n*.tmp"}
              rows={6}
              className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500 resize-y"
              spellCheck={false}
            />
          </>
        )}
      </div>

      {/* Exclude if present */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
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
            value={excludeIfPresentInput}
            onChange={(e) => setExcludeIfPresentInput(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), addExcludeIfPresent())}
            className="flex-1"
          />
          <Button variant="secondary" size="sm" onClick={addExcludeIfPresent}>
            Add
          </Button>
        </div>

        {(() => {
          const v = excludeIfPresentInput.trim();
          if (!v) return null;
          if (v.includes("/") || v.includes("\\")) {
            return (
              <p className="text-xs text-amber-400 mb-2">
                Marker files are matched by name only — remove the path (e.g. use{" "}
                <code className="text-amber-400">.nobackup</code> instead of{" "}
                <code className="text-amber-400">data/.nobackup</code>).
              </p>
            );
          }
          if (v.startsWith("#")) {
            return (
              <p className="text-xs text-amber-400 mb-2">
                Filenames starting with <code className="text-amber-400">#</code> are ignored,
                same as a comment in Exclude Patterns.
              </p>
            );
          }
          return null;
        })()}

        {excludeIfPresent.length > 0 && (
          <ul className="space-y-1.5 mt-2">
            {excludeIfPresent.map((p) => (
              <li
                key={p}
                className="flex items-center justify-between bg-gray-800 rounded-lg px-3 py-2"
              >
                <span className="text-xs font-mono text-gray-300 truncate">{p}</span>
                <button
                  onClick={() => removeExcludeIfPresent(p)}
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

        <label className="flex items-center gap-2 mt-3 pt-3 border-t border-gray-800 cursor-pointer">
          <input
            type="checkbox"
            checked={excludeCaches}
            onChange={(e) => setExcludeCaches(e.target.checked)}
            className="rounded bg-gray-700 border-gray-600"
          />
          <span className="text-sm text-gray-300">
            Also exclude cache directories (<code className="text-gray-400">CACHEDIR.TAG</code>)
          </span>
        </label>
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
                className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
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
                className="w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500"
              />
              <span className="text-xs text-gray-500">KiB/s</span>
            </div>
          ))}
        </div>
      </div>

      {/* Webhooks */}
      <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-6">
        <div className="flex items-center justify-between mb-1">
          <h2 className="text-sm font-medium text-gray-300">Webhooks (optional)</h2>
          <Button variant="secondary" size="sm" onClick={openAddWebhookModal}>+ Add Webhook</Button>
        </div>

        {webhooks.length === 0 ? (
          <p className="text-sm text-gray-500 text-center py-3">
            No webhooks. Add a URL to be notified via Discord, Slack, or any HTTP endpoint.
          </p>
        ) : (
          <ul className="space-y-3 mt-2">
            {webhooks.map((w) => {
              const selectedStages = (["started", "completed", "failed"] as const).filter(
                (s) => w.stages[s],
              );
              return (
                <li
                  key={w.id}
                  // Same row shape as RepositoriesPage's repo cards, one nesting
                  // level down (the card itself is already gray-900).
                  className="flex items-center justify-between p-4 rounded-xl border bg-gray-800 border-gray-700 hover:border-gray-600 transition-colors"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium text-gray-100 truncate">{w.url}</p>
                      <span className="text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded bg-gray-900 border border-gray-700 text-gray-400 flex-shrink-0">
                        {WEBHOOK_PROVIDER_LABELS[w.provider]}
                      </span>
                    </div>
                    {selectedStages.length > 0 ? (
                      <p className="text-xs text-gray-500 mt-0.5">On {selectedStages.join(", ")}</p>
                    ) : (
                      <p className="text-xs text-amber-400 mt-0.5">
                        No stages selected — never fires.
                      </p>
                    )}
                  </div>
                  <div className="flex items-center gap-2 flex-shrink-0 ml-3">
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => openEditWebhookModal(w)}
                      className="text-gray-500 hover:text-blue-400"
                      title="Edit webhook"
                    >
                      <PencilIcon className="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => removeWebhook(w.id)}
                      className="text-gray-500 hover:text-blue-400"
                      title="Remove webhook"
                    >
                      <XIcon className="w-4 h-4" />
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
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
          <Button variant="danger" onClick={() => setConfirmDelete(true)}>
            Delete Plan
          </Button>
        )}
      </div>

      <Modal
        title={editingWebhookId ? "Edit Webhook" : "Add Webhook"}
        open={webhookModalOpen}
        onClose={closeWebhookModal}
      >
        <div className="space-y-4">
          <Input
            label="Endpoint URL"
            placeholder="https://discord.com/api/webhooks/…"
            value={draftUrl}
            onChange={(e) => { setDraftUrl(e.target.value); setWebhookNotice(null); }}
          />

          <div>
            <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
              Payload Format
            </label>
            <select
              value={draftProvider}
              onChange={(e) => { setDraftProvider(e.target.value as WebhookProvider); setWebhookNotice(null); }}
              className="w-full appearance-none bg-gray-800 border border-gray-700 rounded-md px-3 py-2 text-sm text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500"
            >
              <option value="generic">Generic JSON</option>
              <option value="discord">Discord</option>
              <option value="slack">Slack</option>
              <option value="teams">Teams</option>
              <option value="custom">Custom JSON</option>
            </select>
          </div>

          <div>
            <label className="block text-xs font-medium text-gray-400 uppercase tracking-wider mb-2">
              Trigger Stages
            </label>
            <div className="flex flex-wrap gap-4">
              {(["started", "completed", "failed"] as const).map((stage) => (
                <label
                  key={stage}
                  className="flex items-center gap-1.5 text-xs text-gray-400 cursor-pointer"
                >
                  <input
                    type="checkbox"
                    checked={draftStages[stage]}
                    onChange={() => toggleDraftStage(stage)}
                    className="rounded bg-gray-700 border-gray-600"
                  />
                  {stage === "started" ? "Started" : stage === "completed" ? "Completed" : "Failed"}
                </label>
              ))}
            </div>
          </div>

          {draftProvider === "custom" && (
            <div>
              <p className="text-xs text-gray-500 mb-1">
                JSON body sent on each trigger — placeholders:{" "}
                {WEBHOOK_PLACEHOLDERS.join(" ")}
              </p>
              <textarea
                value={draftTemplate}
                onChange={(e) => { setDraftTemplate(e.target.value); setWebhookNotice(null); }}
                rows={4}
                placeholder={DEFAULT_WEBHOOK_TEMPLATE}
                className="w-full bg-gray-800 border border-gray-700 rounded-lg px-3 py-2 text-xs font-mono text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500 resize-y"
                spellCheck={false}
              />
            </div>
          )}

          <div>
            <p className="text-xs text-gray-500 mb-1">Request body preview</p>
            {webhookPreviewLoading && (
              <p className="text-xs text-gray-500">Rendering payload…</p>
            )}
            {!webhookPreviewLoading && webhookPreviewError && (
              <p className="text-xs text-red-300">{webhookPreviewError}</p>
            )}
            {!webhookPreviewLoading && webhookPreview && (
              <>
                {webhookPreview.unknownPlaceholders.length > 0 && (
                  <p className="text-xs text-amber-400 mb-2">
                    Unknown placeholders (sent literally):{" "}
                    {webhookPreview.unknownPlaceholders.join(" ")}
                  </p>
                )}
                {(["started", "completed", "failed"] as const)
                  .filter((stage) => draftStages[stage])
                  .map((stage) => (
                    <div key={stage} className="mb-2 last:mb-0">
                      <p className="text-xs text-gray-500 mb-1">
                        {stage === "started" ? "Started" : stage === "completed" ? "Completed" : "Failed"}
                      </p>
                      <pre className="text-xs font-mono bg-gray-950 border border-gray-700 rounded-lg p-3 overflow-x-auto text-gray-300 whitespace-pre-wrap">
                        {webhookPreview[stage]}
                      </pre>
                    </div>
                  ))}
                {!(draftStages.started || draftStages.completed || draftStages.failed) && (
                  <p className="text-xs text-gray-500">
                    No stages selected — this webhook never fires.
                  </p>
                )}
              </>
            )}
          </div>

          {webhookNotice && (
            <div
              className={`text-sm rounded-lg px-3 py-2 border ${
                webhookNotice.ok
                  ? "bg-green-900/40 text-green-300 border-green-700"
                  : "bg-red-900/40 text-red-300 border-red-700"
              }`}
            >
              {webhookNotice.message}
            </div>
          )}

          <div className="flex items-center justify-between">
            <Button
              variant="secondary"
              loading={testingWebhook}
              onClick={sendTestWebhook}
            >
              Send Test
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" onClick={closeWebhookModal}>Cancel</Button>
              <Button onClick={commitWebhook}>
                {editingWebhookId ? "Save Changes" : "Add Webhook"}
              </Button>
            </div>
          </div>
        </div>
      </Modal>

      <Modal
        title="Delete Backup Plan"
        open={confirmDelete}
        onClose={() => !deleting && setConfirmDelete(false)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-gray-50">{name || "this plan"}</span>?
          This only removes the plan definition — existing snapshots are not affected.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setConfirmDelete(false)} disabled={deleting}>Cancel</Button>
          <Button variant="danger" loading={deleting} onClick={handleDelete}>Delete</Button>
        </div>
      </Modal>
    </div>
  );
}
