import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { getDefaultRetention, listBackupPlans, listRepos, removeBackupPlan, saveBackupPlan } from "../lib/invoke";
import type { Repository } from "../lib/types";
import { parseRetention, retentionToFields } from "../lib/utils";
import {
  PLAN_TABS,
  advancedIsCustom,
  buildPlan,
  effectiveTags,
  emptyForm,
  formFromPlan,
  isPlanTab,
  retentionSummary,
  validatePlanForm,
  type FormIssue,
  type PlanFormState,
  type PlanTab,
} from "../lib/planForm";
import Button from "../components/Button";
import Modal from "../components/Modal";
import AdvancedTab from "../components/plan/AdvancedTab";
import FilesTab from "../components/plan/FilesTab";
import PlanHeader from "../components/plan/PlanHeader";
import PlanTabs, { panelId, tabId, type TabInfo } from "../components/plan/PlanTabs";
import RetentionTab from "../components/plan/RetentionTab";
import TagsTab from "../components/plan/TagsTab";
import type { FormUpdate } from "../components/plan/types";

/**
 * Create / edit a backup plan. The page owns the form state (planForm.ts), loading and saving;
 * everything on screen is a tab (components/plan/) rendering that state, under a sticky header
 * with the editable plan name, the repository and the Save / Cancel / Delete actions.
 */
export default function BackupPlanEditPage() {
  const { planId } = useParams<{ planId: string }>();
  const navigate = useNavigate();
  const isNew = planId === "new";
  const [params, setParams] = useSearchParams();

  const [repos, setRepos] = useState<Repository[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState("");
  const [issue, setIssue] = useState<FormIssue | null>(null);
  const [form, setForm] = useState<PlanFormState>(emptyForm);
  // True when the retention fields were seeded from Settings' default policy (new plans only).
  const [retentionFromDefault, setRetentionFromDefault] = useState(false);

  const update: FormUpdate = useCallback((patch) => {
    setForm((prev) => ({ ...prev, ...(typeof patch === "function" ? patch(prev) : patch) }));
  }, []);

  // The selected tab lives in the URL (?tab=retention) so it survives a reload and can be linked
  // to. A tab is mounted the first time it is shown and then kept (hidden when inactive), so
  // what was typed in it, and the file tree's state, survive a switch.
  const urlTab = params.get("tab");
  const tab: PlanTab = isPlanTab(urlTab) ? urlTab : "files";
  const [visited, setVisited] = useState<Set<PlanTab>>(() => new Set([tab]));
  useEffect(() => {
    setVisited((prev) => (prev.has(tab) ? prev : new Set(prev).add(tab)));
  }, [tab]);
  const selectTab = useCallback(
    (t: PlanTab) => {
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("tab", t);
          return next;
        },
        { replace: true },
      );
      // Opening the tab that had the problem acknowledges it.
      setIssue((cur) => (cur?.where === t ? null : cur));
    },
    [setParams],
  );

  useEffect(() => {
    const init = async () => {
      setLoading(true);
      try {
        const [allRepos, allPlans] = await Promise.all([listRepos(), listBackupPlans()]);
        setRepos(allRepos);

        if (!isNew) {
          const plan = allPlans.find((p) => p.id === planId);
          if (plan) {
            const repoStillExists = allRepos.some((r) => r.id === plan.repoId);
            setForm(formFromPlan(plan, repoStillExists));
            if (!repoStillExists) {
              setError("The repository linked to this plan no longer exists. Please select a new one.");
            }
          } else {
            setError("Backup plan not found.");
          }
        } else {
          if (allRepos.length > 0) setForm((prev) => ({ ...prev, repoId: allRepos[0].id }));
          // New plans only: seed retention from Settings' default policy. A failed load prefills
          // nothing — the safe direction, since retention deletes snapshots.
          const def = await getDefaultRetention().catch(() => null);
          if (def?.enabled) {
            const fields = retentionToFields(def.policy);
            const parsed = parseRetention(fields);
            if ("value" in parsed && parsed.value) {
              setForm((prev) => ({ ...prev, retention: fields }));
              setRetentionFromDefault(true);
            }
          }
        }
      } catch (err: any) {
        setError(String(err));
      } finally {
        setLoading(false);
      }
    };
    init();
  }, [planId, isNew]);

  const handleSave = async () => {
    const problem = validatePlanForm(form);
    if (problem) {
      setError(problem.message);
      setIssue(problem);
      if (problem.where === "header") {
        document.getElementById(form.name.trim() ? "plan-repo" : "plan-name")?.focus();
      } else {
        selectTab(problem.where);
        // selectTab acknowledges the issue; keep the dot until the user edits or saves again.
        setIssue(problem);
      }
      return;
    }

    setSaving(true);
    setError("");
    setIssue(null);
    try {
      await saveBackupPlan(buildPlan(form, isNew ? crypto.randomUUID() : planId!));
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

  const tags = effectiveTags(form);
  const tabInfo: Partial<Record<PlanTab, TabInfo>> = {
    files: { alert: issue?.where === "files" },
    retention: { summary: retentionSummary(form), alert: issue?.where === "retention" },
    advanced: { dot: advancedIsCustom(form), alert: issue?.where === "advanced" },
    tags: { summary: tags.length ? `${tags.length} ${tags.length === 1 ? "tag" : "tags"}` : undefined, alert: issue?.where === "tags" },
  };

  const content: Record<PlanTab, React.ReactNode> = {
    files: <FilesTab form={form} update={update} active={tab === "files"} />,
    retention: <RetentionTab form={form} update={update} seededFromDefault={retentionFromDefault} />,
    advanced: <AdvancedTab form={form} update={update} />,
    tags: <TagsTab form={form} update={update} />,
  };

  return (
    <div>
      <PlanHeader
        name={form.name}
        onName={(name) => {
          update({ name });
          if (issue?.where === "header") setIssue(null);
        }}
        nameInvalid={issue?.where === "header" && issue.message === "Plan name is required."}
        repoInvalid={issue?.where === "header" && issue.message === "Select a target repository."}
        repos={repos}
        repoId={form.repoId}
        onRepo={(repoId) => {
          update({ repoId });
          setError("");
          if (issue?.where === "header") setIssue(null);
        }}
        isNew={isNew}
        saving={saving}
        onSave={handleSave}
        onCancel={() => navigate("/backup-plans")}
        onDelete={() => setConfirmDelete(true)}
      />

      <div className="px-6 pt-4 pb-8">
        {error && (
          <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300" role="alert">
            {error}
          </div>
        )}

        <PlanTabs active={tab} onChange={selectTab} info={tabInfo} />

        {PLAN_TABS.filter((t) => visited.has(t)).map((t) => (
          <div key={t} id={panelId(t)} role="tabpanel" aria-labelledby={tabId(t)} hidden={t !== tab}>
            {content[t]}
          </div>
        ))}
      </div>

      <Modal
        title="Delete Backup Plan"
        open={confirmDelete}
        onClose={() => !deleting && setConfirmDelete(false)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to delete{" "}
          <span className="font-semibold text-gray-50">{form.name || "this plan"}</span>?
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
