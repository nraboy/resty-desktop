// The backup-plan editor's form, as pure functions: the state shape, loading a saved plan into
// it, validating it (naming the place an error belongs to — the header, the Files tab or a
// Settings section — so the page can jump there), building the plan to save, where the URL
// points (tab and section), and which Settings sections are set away from their defaults. The
// page and its components only render this state; nothing here touches React or the backend, so
// the rules that decide what gets saved are unit-tested (planForm.test.ts).

import type { BackupPlan, ExcludeFile, PlanWebhook } from "./types";
import type { TreeRulesInput } from "./planTree";
import { EMPTY_RETENTION_FIELDS, parsePackSize, parseRetention, retentionToFields, type RetentionFields } from "./utils";

export type PlanTab = "files" | "settings";
export const PLAN_TABS: PlanTab[] = ["files", "settings"];
export const PLAN_TAB_LABELS: Record<PlanTab, string> = {
  files: "Files",
  settings: "Settings",
};

export function isPlanTab(v: string | null | undefined): v is PlanTab {
  return !!v && (PLAN_TABS as string[]).includes(v);
}

/** The Settings tab's sections, top to bottom: one scrolling pane with a jump list beside it. */
export type SettingsSection = "retention" | "tags" | "bandwidth" | "restic" | "webhooks";
export const SETTINGS_SECTIONS: SettingsSection[] = ["retention", "tags", "bandwidth", "restic", "webhooks"];
export const SETTINGS_SECTION_LABELS: Record<SettingsSection, string> = {
  retention: "Retention",
  tags: "Tags",
  bandwidth: "Bandwidth",
  restic: "Restic options",
  webhooks: "Webhooks",
};

export function isSettingsSection(v: string | null | undefined): v is SettingsSection {
  return !!v && (SETTINGS_SECTIONS as string[]).includes(v);
}

/** Tabs the editor used to have, and the Settings section each became — so a saved or shared
 * `?tab=retention|advanced|tags` link still opens the right place. */
const OLD_TABS: Record<string, SettingsSection> = {
  retention: "retention",
  tags: "tags",
  advanced: "bandwidth",
};

/** Where the URL's `?tab=` and `&section=` point. Anything unknown falls back rather than
 * failing: an unknown tab opens Files, an unknown section opens Settings at the top, and a
 * section only means something on the Settings tab. */
export function planLocation(
  tab: string | null,
  section: string | null,
): { tab: PlanTab; section: SettingsSection | null } {
  if (tab === "settings") return { tab, section: isSettingsSection(section) ? section : null };
  if (tab && Object.prototype.hasOwnProperty.call(OLD_TABS, tab)) return { tab: "settings", section: OLD_TABS[tab] };
  return { tab: "files", section: null };
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
  /** The five retention inputs as typed (blank = unset); see `parseRetention`. */
  retention: RetentionFields;
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
    retention: { ...EMPTY_RETENTION_FIELDS },
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
    retention: retentionToFields(plan.retention),
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

/** Whether an exclude line is a `#` comment (restic and the backend ignore it; the editor keeps it). */
export function isCommentLine(line: string): boolean {
  return line.trim().startsWith("#");
}

/** The expert-mode textarea as the lines it saves: trimmed, blank lines dropped, comments and
 * repeats kept, in order (`X`, `!X`, `X` means something). */
export function expertLines(text: string): string[] {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => l !== "");
}

/** The exclude lines that will be saved, in order. */
export function savedExcludes(f: PlanFormState): string[] {
  return f.excludeMode === "expert" ? expertLines(f.excludeText) : f.excludeItems;
}

/** What changes when the exclude editor switches view: the list becomes the text, and the text
 * folds back into the list line for line, so `excludeItems` is the source of truth whenever
 * Expert isn't active. Nothing is de-duplicated or dropped but blank lines. */
export function excludesAfterModeSwitch(
  f: Pick<PlanFormState, "excludeItems" | "excludeText">,
  mode: ExcludeMode,
): Pick<PlanFormState, "excludeMode"> & Partial<Pick<PlanFormState, "excludeItems" | "excludeText">> {
  return mode === "expert"
    ? { excludeMode: mode, excludeText: f.excludeItems.join("\n") }
    : { excludeMode: mode, excludeItems: expertLines(f.excludeText) };
}

/** The list with `input` appended, or null when there's nothing to add: a blank entry, or a
 * pattern already listed. A comment may repeat — two notes can say the same thing. */
export function withExcludeAdded(items: string[], input: string): string[] | null {
  const v = input.trim();
  if (!v || (!isCommentLine(v) && items.includes(v))) return null;
  return [...items, v];
}

const toNum = (s: string): number | undefined => {
  if (s.trim() === "") return undefined;
  const n = parseInt(s, 10);
  return Number.isNaN(n) ? undefined : n;
};

export function hasRetention(f: PlanFormState): boolean {
  return Object.values(f.retention).some((s) => s.trim() !== "");
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
 * area above the tabs, `files` the Files tab, anything else a Settings section. */
export interface FormIssue {
  where: "header" | "files" | SettingsSection;
  message: string;
}

/** The tab a problem is on (the header is above both). */
export function issueTab(where: FormIssue["where"]): PlanTab | null {
  if (where === "header") return null;
  return where === "files" ? "files" : "settings";
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
    return { where: "webhooks", message: `Webhook "${badUrl.url}" must start with http:// or https://.` };
  }
  const pack = parsePackSize(f.packSize);
  if ("error" in pack) return { where: "restic", message: pack.error };
  // 0 means "unset" throughout the app (restic rejects a forget with no effective policy), so a
  // 0/negative/decimal/non-numeric count is an error here rather than a silently dead rule.
  const retention = parseRetention(f.retention);
  if ("error" in retention) return { where: "retention", message: retention.error };
  return null;
}

/** The plan to save. Call after `validatePlanForm` returned null. */
export function buildPlan(f: PlanFormState, id: string): BackupPlan {
  const pack = parsePackSize(f.packSize);
  const retention = parseRetention(f.retention);
  return {
    id,
    name: f.name.trim(),
    repoId: f.repoId,
    paths: f.paths,
    tags: effectiveTags(f),
    excludes: savedExcludes(f),
    excludeIfPresent: f.excludeIfPresent,
    excludeCaches: f.excludeCaches,
    retention: "value" in retention ? retention.value : undefined,
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

// ── non-default markers ─────────────────────────────────────────────────────

/** Is a Settings section set away from its default? (A blue dot beside it in the jump list.) */
export function sectionIsCustom(f: PlanFormState, s: SettingsSection): boolean {
  switch (s) {
    case "retention":
      return hasRetention(f);
    case "tags":
      return effectiveTags(f).length > 0;
    case "bandwidth":
      return f.limitUpload.trim() !== "" || f.limitDownload.trim() !== "";
    case "restic":
      return f.packSize.trim() !== "" || f.excludeCloudFiles || f.useFsSnapshot;
    case "webhooks":
      return f.webhooks.length > 0;
  }
}

/** Is any Settings section set away from its default? (A blue dot on the Settings tab.) */
export function settingsIsCustom(f: PlanFormState): boolean {
  return SETTINGS_SECTIONS.some((s) => sectionIsCustom(f, s));
}
