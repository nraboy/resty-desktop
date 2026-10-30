import { useCallback, useEffect, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { getDefaultRetention, listBackupPlans, listRepos, removeBackupPlan, saveBackupPlan } from "../lib/invoke";
import type { Repository } from "../lib/types";
import { parseRetention, retentionToFields } from "../lib/utils";
import {
  buildPlan,
  emptyForm,
  formFromPlan,
  issueTab,
  planLocation,
  settingsIsCustom,
  validatePlanForm,
  type FormIssue,
  type PlanFormState,
  type PlanTab,
  type SettingsSection,
} from "../lib/planForm";
import Button from "../components/Button";
import Modal from "../components/Modal";
import FilesTab from "../components/plan/FilesTab";
import PlanFooter from "../components/plan/PlanFooter";
import PlanHeader from "../components/plan/PlanHeader";
import PlanTabs, { panelId, tabId, type TabInfo } from "../components/plan/PlanTabs";
import SettingsTab from "../components/plan/SettingsTab";
import type { FormUpdate } from "../components/plan/types";

/**
 * Create / edit a backup plan. The page owns the form state (planForm.ts), loading and saving.
 * It is a full-height column: the header (editable plan name, repository), the Files | Settings
 * tab strip, the active tab's content — the only part that scrolls — and a footer with the
 * Delete / Cancel / Save actions and the last error.
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
  // Bumped to make the Settings pane scroll to the URL's section again when it hasn't changed.
  const [sectionRequest, setSectionRequest] = useState(0);

  // Any edit acknowledges the last save's problem: its red dots go (the footer message stays
  // until the next save, as it describes that attempt).
  const update: FormUpdate = useCallback((patch) => {
    setForm((prev) => ({ ...prev, ...(typeof patch === "function" ? patch(prev) : patch) }));
    setIssue(null);
  }, []);

  // Where the editor is lives in the URL (?tab=settings&section=webhooks) so it survives a
  // reload and can be linked to; old ?tab=retention|advanced|tags links open Settings at the
  // matching section. A tab is mounted the first time it is shown and then kept (hidden when
  // inactive), so what was typed in it, and the file tree's state, survive a switch.
  const { tab, section } = planLocation(params.get("tab"), params.get("section"));
  const [visited, setVisited] = useState<Set<PlanTab>>(() => new Set([tab]));
  useEffect(() => {
    setVisited((prev) => (prev.has(tab) ? prev : new Set(prev).add(tab)));
  }, [tab]);

  const go = useCallback(
    (t: PlanTab, s?: SettingsSection) => {
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("tab", t);
          if (s) next.set("section", s);
          return next;
        },
        { replace: true },
      );
    },
    [setParams],
  );

  const selectTab = useCallback(
    (t: PlanTab) => {
      go(t);
      // Opening the Files tab when that's where the problem was acknowledges it. (Opening
      // Settings doesn't: the jump list's red dot still has to point at the section.)
      setIssue((cur) => (cur?.where === t ? null : cur));
    },
    [go],
  );

  const jumpToSection = useCallback(
    (s: SettingsSection) => {
      go("settings", s);
      setIssue((cur) => (cur?.where === s ? null : cur));
    },
    [go],
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
      // Kept after the jump below: the dots stay until the user edits, saves again, or opens
      // the place the problem is.
      setIssue(problem);
      if (problem.where === "header") {
        document.getElementById(form.name.trim() ? "plan-repo" : "plan-name")?.focus();
      } else if (problem.where === "files") {
        go("files");
      } else {
        go("settings", problem.where);
        // Scroll there even when the URL already names this section (the user may have
        // scrolled away from it since).
        setSectionRequest((n) => n + 1);
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

  const alertTab = issue ? issueTab(issue.where) : null;
  const tabInfo: Partial<Record<PlanTab, TabInfo>> = {
    files: { alert: alertTab === "files" },
    settings: { dot: settingsIsCustom(form), alert: alertTab === "settings" },
  };
  const alertSection: SettingsSection | null =
    issue && issue.where !== "header" && issue.where !== "files" ? issue.where : null;

  return (
    <div className="flex flex-col h-full">
      <PlanHeader
        name={form.name}
        onName={(name) => update({ name })}
        nameInvalid={issue?.where === "header" && issue.message === "Plan name is required."}
        repoInvalid={issue?.where === "header" && issue.message === "Select a target repository."}
        repos={repos}
        repoId={form.repoId}
        onRepo={(repoId) => {
          update({ repoId });
          setError("");
        }}
      />

      <PlanTabs active={tab} onChange={selectTab} info={tabInfo} />

      <div className="flex-1 min-h-0">
        {visited.has("files") && (
          <div
            id={panelId("files")}
            role="tabpanel"
            aria-labelledby={tabId("files")}
            hidden={tab !== "files"}
            className="h-full overflow-y-auto px-6 pt-4 pb-6"
          >
            <FilesTab form={form} update={update} active={tab === "files"} />
          </div>
        )}
        {visited.has("settings") && (
          <div
            id={panelId("settings")}
            role="tabpanel"
            aria-labelledby={tabId("settings")}
            hidden={tab !== "settings"}
            className="h-full"
          >
            <SettingsTab
              form={form}
              update={update}
              seededFromDefault={retentionFromDefault}
              requested={section}
              requestKey={sectionRequest}
              onJump={jumpToSection}
              alertSection={alertSection}
            />
          </div>
        )}
      </div>

      <PlanFooter
        isNew={isNew}
        saving={saving}
        error={error}
        onSave={handleSave}
        onCancel={() => navigate("/backup-plans")}
        onDelete={() => setConfirmDelete(true)}
      />

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
