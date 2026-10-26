// The backup-plan editor's form, as pure functions: the state shape, loading a saved plan into
// it, validating it (naming the tab an error belongs to, so the page can jump there), building
// the plan to save, and the short summaries shown on the tabs. The page and its tab components
// only render this state; nothing here touches React or the backend, so the rules that decide
// what gets saved are unit-tested (planForm.test.ts).

import type { BackupPlan, ExcludeFile, PlanWebhook } from "./types";
import type { TreeRulesInput } from "./planTree";
import { parsePackSize } from "./utils";

export type PlanTab = "files" | "retention" | "advanced" | "tags";
export const PLAN_TABS: PlanTab[] = ["files", "retention", "advanced", "tags"];
export const PLAN_TAB_LABELS: Record<PlanTab, string> = {
  files: "Files",
  retention: "Retention",
  advanced: "Advanced",
  tags: "Tags",
};

export function isPlanTab(v: string | null | undefined): v is PlanTab {
  return !!v && (PLAN_TABS as string[]).includes(v);
}

export type ExcludeMode = "simple" | "expert";

/** Everything the editor holds. Typed-but-uncommitted text boxes that don't affect saving
 * (the "add exclude" input, say) live in the components that own them. */
export interface PlanFormState {
  name: string;
  repoId: string;
  /** Folders and files to back up, edited from the tree. */
  paths: string[];
  /** Text files listing more paths (`--files-from`); combinable with `paths`. */
  filesFrom: string[];
  excludeFiles: ExcludeFile[];
  tags: string[];
  /** A tag typed but not added yet still counts on save. */
  tagInput: string;
  excludeMode: ExcludeMode;
  excludeItems: string[];
  excludeText: string;
  excludeIfPresent: string[];
  excludeCaches: boolean;
  keepLast: string;
  keepDaily: string;
  keepWeekly: string;
  keepMonthly: string;
  keepYearly: string;
  limitUpload: string;
  limitDownload: string;
  packSize: string;
  /** Windows-only; always loaded and saved so editing an imported plan on another OS never
   * clears them. */
  excludeCloudFiles: boolean;
  useFsSnapshot: boolean;
  webhooks: PlanWebhook[];
}

export function emptyForm(): PlanFormState {
  return {
    name: "",
    repoId: "",
    paths: [],
    filesFrom: [],
    excludeFiles: [],
    tags: [],
    tagInput: "",
    excludeMode: "simple",
    excludeItems: [],
    excludeText: "",
    excludeIfPresent: [],
    excludeCaches: false,
    keepLast: "",
    keepDaily: "",
    keepWeekly: "",
    keepMonthly: "",
    keepYearly: "",
    limitUpload: "",
    limitDownload: "",
    packSize: "",
    excludeCloudFiles: false,
    useFsSnapshot: false,
    webhooks: [],
  };
}

/** The form for a saved plan. `repoStillExists` false clears the repository (the page then asks
 * for a new one). */
export function formFromPlan(plan: BackupPlan, repoStillExists: boolean): PlanFormState {
  const filesFrom = plan.filesFrom ?? [];
  return {
    ...emptyForm(),
    name: plan.name,
    repoId: repoStillExists ? plan.repoId : "",
    paths: plan.paths,
    filesFrom,
    excludeFiles: plan.excludeFiles ?? [],
    tags: plan.tags,
    excludeItems: plan.excludes,
    excludeText: plan.excludes.join("\n"),
    excludeIfPresent: plan.excludeIfPresent,
    excludeCaches: plan.excludeCaches,
    keepLast: plan.retention?.keepLast?.toString() ?? "",
    keepDaily: plan.retention?.keepDaily?.toString() ?? "",
    keepWeekly: plan.retention?.keepWeekly?.toString() ?? "",
    keepMonthly: plan.retention?.keepMonthly?.toString() ?? "",
    keepYearly: plan.retention?.keepYearly?.toString() ?? "",
    limitUpload: plan.limitUpload?.toString() ?? "",
    limitDownload: plan.limitDownload?.toString() ?? "",
    packSize: plan.packSize?.toString() ?? "",
    excludeCloudFiles: plan.excludeCloudFiles ?? false,
    useFsSnapshot: plan.useFsSnapshot ?? false,
    webhooks: plan.webhooks ?? [],
  };
}

/** The tags that will be saved: the list plus a typed-but-not-added tag. State is left alone,
 * so a failed save leaves the form exactly as the user had it. */
export function effectiveTags(f: PlanFormState): string[] {
  const pending = f.tagInput.trim();
  return pending && !f.tags.includes(pending) ? [...f.tags, pending] : f.tags;
}

/** The expert-mode textarea as the patterns it saves: trimmed, blanks and `#` comments dropped. */
export function expertPatterns(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith("#"));
}

/** The exclude patterns that will be saved, in order. */
export function savedExcludes(f: PlanFormState): string[] {
  return f.excludeMode === "expert" ? expertPatterns(f.excludeText) : f.excludeItems;
}

const toNum = (s: string): number | undefined => {
  if (s.trim() === "") return undefined;
  const n = parseInt(s, 10);
  return Number.isNaN(n) ? undefined : n;
};

export function hasRetention(f: PlanFormState): boolean {
  return [f.keepLast, f.keepDaily, f.keepWeekly, f.keepMonthly, f.keepYearly].some((s) => s.trim() !== "");
}

function retentionOf(f: PlanFormState): BackupPlan["retention"] {
  if (!hasRetention(f)) return undefined;
  return {
    keepLast: toNum(f.keepLast),
    keepDaily: toNum(f.keepDaily),
    keepWeekly: toNum(f.keepWeekly),
    keepMonthly: toNum(f.keepMonthly),
    keepYearly: toNum(f.keepYearly),
  };
}

/** What the editor's preview tree needs to decide what's included: the sources and every
 * exclusion the plan will pass to restic, exactly as they'd be saved. */
export function treeRulesFromForm(f: PlanFormState): TreeRulesInput {
  return {
    sources: f.paths,
    // Expert text goes in raw (comments and blanks included): edits made from the tree return it
    // whole, so the user's text survives.
    excludes: f.excludeMode === "expert" ? f.excludeText.split("\n") : f.excludeItems,
    excludeIfPresent: f.excludeIfPresent,
    excludeCaches: f.excludeCaches,
    excludeFiles: f.excludeFiles,
    filesFrom: f.filesFrom,
  };
}

/** A reason the form can't be saved, and where to fix it: `header` is the title/repository
 * area above the tabs. */
export interface FormIssue {
  where: PlanTab | "header";
  message: string;
}

/** The first problem with the form, in the order the editor has always checked them. */
export function validatePlanForm(f: PlanFormState): FormIssue | null {
  if (!f.name.trim()) return { where: "header", message: "Plan name is required." };
  if (!f.repoId) return { where: "header", message: "Select a target repository." };
  if (f.paths.length === 0 && f.filesFrom.length === 0) {
    return { where: "files", message: "Pick at least one folder or file to back up." };
  }
  if (f.filesFrom.length > 0 && effectiveTags(f).length === 0) {
    return {
      where: "tags",
      message: "Add at least one tag — a plan that uses path-list files needs one so its snapshots can be identified.",
    };
  }
  // The webhook modal enforces this for every row it creates; this only catches rows that
  // arrived from outside the UI (a hand-edited import bundle).
  const badUrl = f.webhooks.find((w) => !/^https?:\/\//.test(w.url.trim()));
  if (badUrl) {
    return { where: "advanced", message: `Webhook "${badUrl.url}" must start with http:// or https://.` };
  }
  const pack = parsePackSize(f.packSize);
  if ("error" in pack) return { where: "advanced", message: pack.error };
  // 0 means "unset" throughout the app (restic rejects a forget with no effective policy), so a
  // 0/negative/non-numeric count is an error here rather than a silently dead rule.
  const retention = retentionOf(f);
  if (retention) {
    const counts = Object.values(retention);
    if (counts.every((v) => v === undefined) || counts.some((v) => v !== undefined && v < 1)) {
      return {
        where: "retention",
        message: "Retention counts must be whole numbers of at least 1 — leave a field blank to disable it.",
      };
    }
  }
  return null;
}

/** The plan to save. Call after `validatePlanForm` returned null. */
export function buildPlan(f: PlanFormState, id: string): BackupPlan {
  const pack = parsePackSize(f.packSize);
  return {
    id,
    name: f.name.trim(),
    repoId: f.repoId,
    paths: f.paths,
    tags: effectiveTags(f),
    excludes: savedExcludes(f),
    excludeIfPresent: f.excludeIfPresent,
    excludeCaches: f.excludeCaches,
    retention: retentionOf(f),
    limitUpload: toNum(f.limitUpload),
    limitDownload: toNum(f.limitDownload),
    webhooks: f.webhooks,
    packSize: "value" in pack ? pack.value : undefined,
    excludeCloudFiles: f.excludeCloudFiles,
    useFsSnapshot: f.useFsSnapshot,
    filesFrom: f.filesFrom,
    excludeFiles: f.excludeFiles,
  };
}

// ── tab summaries ───────────────────────────────────────────────────────────

/** "Off", or e.g. "Keep 7 daily, 4 weekly" — shown on the Retention tab. */
export function retentionSummary(f: PlanFormState): string {
  const parts: string[] = [];
  const add = (v: string, label: string) => {
    if (v.trim() !== "") parts.push(`${v.trim()} ${label}`);
  };
  add(f.keepLast, "last");
  add(f.keepDaily, "daily");
  add(f.keepWeekly, "weekly");
  add(f.keepMonthly, "monthly");
  add(f.keepYearly, "yearly");
  return parts.length ? `Keep ${parts.join(", ")}` : "Off";
}

/** Is anything on the Advanced tab set away from its default? (A dot on the tab.) */
export function advancedIsCustom(f: PlanFormState): boolean {
  return (
    f.packSize.trim() !== "" ||
    f.limitUpload.trim() !== "" ||
    f.limitDownload.trim() !== "" ||
    f.webhooks.length > 0 ||
    f.excludeCloudFiles ||
    f.useFsSnapshot
  );
}
