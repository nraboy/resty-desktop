import { describe, it, expect } from "vitest";
import {
  PLAN_TABS,
  PLAN_TAB_LABELS,
  SETTINGS_SECTIONS,
  SETTINGS_SECTION_LABELS,
  buildPlan,
  effectiveTags,
  emptyForm,
  excludesAfterModeSwitch,
  expertLines,
  formFromPlan,
  hasRetention,
  isCommentLine,
  isPlanTab,
  isSettingsSection,
  issueTab,
  planLocation,
  savedExcludes,
  sectionIsCustom,
  settingsIsCustom,
  treeRulesFromForm,
  validatePlanForm,
  withExcludeAdded,
  type PlanFormState,
} from "./planForm";
import type { BackupPlan } from "./types";

const keep = (over: Partial<PlanFormState["retention"]>): PlanFormState["retention"] => ({ ...emptyForm().retention, ...over });

const valid = (over: Partial<PlanFormState> = {}): PlanFormState => ({
  ...emptyForm(),
  name: "Docs",
  repoId: "r1",
  paths: ["/home/u/docs"],
  ...over,
});

const plan = (over: Partial<BackupPlan> = {}): BackupPlan => ({
  id: "p1",
  name: "Docs",
  repoId: "r1",
  paths: ["/home/u/docs"],
  tags: ["daily"],
  excludes: ["node_modules", "*.log"],
  excludeIfPresent: [".nobackup"],
  excludeCaches: true,
  retention: { keepDaily: 7, keepWeekly: 4 },
  limitUpload: 500,
  limitDownload: undefined,
  webhooks: [],
  packSize: 32,
  excludeCloudFiles: false,
  useFsSnapshot: false,
  filesFrom: [],
  excludeFiles: [{ path: "/home/u/.ignore", ignoreCase: true }],
  ...over,
});

describe("validatePlanForm", () => {
  it("accepts a plan with a name, repository and a source", () => {
    expect(validatePlanForm(valid())).toBeNull();
  });

  it("checks in the editor's long-standing order, each naming where to fix it", () => {
    expect(validatePlanForm(valid({ name: "  " }))).toEqual({ where: "header", message: "Plan name is required." });
    expect(validatePlanForm(valid({ repoId: "" }))).toEqual({ where: "header", message: "Select a target repository." });
    expect(validatePlanForm(valid({ paths: [] }))).toEqual({ where: "files", message: "Pick at least one folder or file to back up." });
    // Name is checked before the repository, and both before sources.
    expect(validatePlanForm(valid({ name: "", repoId: "", paths: [] }))?.message).toBe("Plan name is required.");
  });

  it("needs at least one source — a path or a list file — and a tag whenever list files are used", () => {
    expect(validatePlanForm(valid({ paths: [], filesFrom: [] }))).toEqual({
      where: "files",
      message: "Pick at least one folder or file to back up.",
    });
    const noTag = validatePlanForm(valid({ paths: [], filesFrom: ["/l.txt"] }));
    expect(noTag?.where).toBe("tags");
    expect(noTag?.message).toContain("needs one so its snapshots can be identified");
    // A typed-but-not-added tag counts.
    expect(validatePlanForm(valid({ paths: [], filesFrom: ["/l.txt"], tagInput: " nightly " }))).toBeNull();
    expect(validatePlanForm(valid({ paths: [], filesFrom: ["/l.txt"], tags: ["t"] }))).toBeNull();
  });

  it("lets paths and list files be combined, and only asks for a tag when list files are present", () => {
    expect(validatePlanForm(valid({ paths: ["/x"], filesFrom: [] }))).toBeNull();
    expect(validatePlanForm(valid({ paths: ["/x"], filesFrom: ["/l.txt"] }))?.where).toBe("tags");
    expect(validatePlanForm(valid({ paths: ["/x"], filesFrom: ["/l.txt"], tags: ["t"] }))).toBeNull();
  });

  it("rejects a webhook that isn't http(s) in Webhooks, and a bad pack size in Restic options", () => {
    const hook = (url: string) => ({ id: "w", url, provider: "generic" as const, stages: { started: false, completed: true, failed: true } });
    expect(validatePlanForm(valid({ webhooks: [hook("ftp://x")] }))).toEqual({
      where: "webhooks",
      message: 'Webhook "ftp://x" must start with http:// or https://.',
    });
    expect(validatePlanForm(valid({ webhooks: [hook(" https://ok ")] }))).toBeNull();
    const pack = validatePlanForm(valid({ packSize: "3" }));
    expect(pack?.where).toBe("restic");
    expect(pack?.message).toContain("between");
    expect(validatePlanForm(valid({ packSize: "64" }))).toBeNull();
  });

  it("rejects retention counts below 1 or non-numeric in the Retention section", () => {
    for (const bad of ["0", "-2", "abc", "1.5", "1e2"]) {
      expect(validatePlanForm(valid({ retention: keep({ keepDaily: bad }) }))).toEqual({
        where: "retention",
        message: "Retention counts must be whole numbers of at least 1 — leave a field blank to disable it.",
      });
    }
    expect(validatePlanForm(valid({ retention: keep({ keepDaily: "7", keepWeekly: "0" }) }))?.where).toBe("retention");
    expect(validatePlanForm(valid({ retention: keep({ keepDaily: "7" }) }))).toBeNull();
    expect(validatePlanForm(valid({ retention: keep({ keepDaily: "  " }) }))).toBeNull();
  });

  it("reports the first problem when several sections have one", () => {
    expect(validatePlanForm(valid({ packSize: "1", retention: keep({ keepDaily: "0" }) }))?.where).toBe("restic");
  });
});

describe("buildPlan", () => {
  it("maps a simple form to a plan", () => {
    const p = buildPlan(
      valid({
        name: "  Docs ",
        tags: ["a"],
        excludeItems: ["x", "!x/keep"],
        excludeIfPresent: [".nobackup"],
        excludeCaches: true,
        retention: keep({ keepDaily: "7" }),
        limitUpload: "500",
        packSize: "32",
        excludeFiles: [{ path: "/e.txt", ignoreCase: false }],
      }),
      "id1",
    );
    expect(p).toMatchObject({
      id: "id1",
      name: "Docs",
      repoId: "r1",
      paths: ["/home/u/docs"],
      tags: ["a"],
      excludes: ["x", "!x/keep"],
      excludeIfPresent: [".nobackup"],
      excludeCaches: true,
      retention: { keepDaily: 7 },
      limitUpload: 500,
      limitDownload: undefined,
      packSize: 32,
      filesFrom: [],
      excludeFiles: [{ path: "/e.txt", ignoreCase: false }],
    });
  });

  it("saves paths and list files together", () => {
    const f = valid({ paths: ["/a"], filesFrom: ["/l.txt"], tags: ["t"] });
    expect(buildPlan(f, "i")).toMatchObject({ paths: ["/a"], filesFrom: ["/l.txt"] });
  });

  it("saves expert text trimmed, in order, keeping comments and dropping blank lines", () => {
    const f = valid({ excludeMode: "expert", excludeText: "  node_modules \n# note\n\n!keep\n*.log" });
    expect(buildPlan(f, "i").excludes).toEqual(["node_modules", "# note", "!keep", "*.log"]);
    expect(savedExcludes(f)).toEqual(["node_modules", "# note", "!keep", "*.log"]);
  });

  it("saves the simple list as is: comments, repeats and order included (order is meaning)", () => {
    expect(savedExcludes(valid({ excludeItems: ["# why", "!x", "y", "!x"] }))).toEqual(["# why", "!x", "y", "!x"]);
  });

  it("keeps comments and repeated rules through load and save", () => {
    const excludes = ["# Tree: excluded /a", "/a", "# Tree: included /a/keep", "!/a/keep", "/a", "# my note"];
    expect(buildPlan(formFromPlan(plan({ excludes }), true), "p1").excludes).toEqual(excludes);
    const expert = { ...formFromPlan(plan({ excludes }), true), excludeMode: "expert" as const };
    expect(buildPlan(expert, "p1").excludes).toEqual(excludes);
  });

  it("includes a typed-but-not-added tag, once", () => {
    expect(effectiveTags(valid({ tags: ["a"], tagInput: " b " }))).toEqual(["a", "b"]);
    expect(effectiveTags(valid({ tags: ["a"], tagInput: "a" }))).toEqual(["a"]);
    expect(effectiveTags(valid({ tags: ["a"], tagInput: "  " }))).toEqual(["a"]);
    expect(buildPlan(valid({ tagInput: "t" }), "i").tags).toEqual(["t"]);
  });

  it("has no retention when every count is blank, and parses the ones set", () => {
    expect(buildPlan(valid(), "i").retention).toBeUndefined();
    expect(hasRetention(valid({ retention: keep({ keepYearly: "2" }) }))).toBe(true);
    expect(buildPlan(valid({ retention: keep({ keepLast: "3", keepYearly: "2" }) }), "i").retention).toEqual({
      keepLast: 3,
      keepDaily: undefined,
      keepWeekly: undefined,
      keepMonthly: undefined,
      keepYearly: 2,
    });
  });

  it("keeps the Windows-only flags and webhooks whatever the platform", () => {
    const f = valid({ excludeCloudFiles: true, useFsSnapshot: true });
    expect(buildPlan(f, "i")).toMatchObject({ excludeCloudFiles: true, useFsSnapshot: true });
  });
});

describe("formFromPlan", () => {
  it("round-trips a saved plan through the form unchanged", () => {
    const p = plan();
    const back = buildPlan(formFromPlan(p, true), p.id);
    expect(back).toEqual(p);
  });

  it("clears the repository when it no longer exists", () => {
    expect(formFromPlan(plan(), false).repoId).toBe("");
    expect(formFromPlan(plan(), true).repoId).toBe("r1");
  });

  it("keeps both kinds of source, and tolerates a plan saved before list files existed", () => {
    const both = formFromPlan(plan({ paths: ["/a"], filesFrom: ["/l.txt"] }), true);
    expect(both.paths).toEqual(["/a"]);
    expect(both.filesFrom).toEqual(["/l.txt"]);
    expect(formFromPlan(plan({ filesFrom: undefined as unknown as string[] }), true).filesFrom).toEqual([]);
  });

  it("fills the expert text from the patterns and stringifies numbers", () => {
    const f = formFromPlan(plan(), true);
    expect(f.excludeText).toBe("node_modules\n*.log");
    expect(f.retention.keepDaily).toBe("7");
    expect(f.packSize).toBe("32");
    expect(f.limitDownload).toBe("");
  });
});

describe("expertLines and isCommentLine", () => {
  it("expertLines trims, drops blank lines and keeps comments", () => {
    expect(expertLines("a\n#b\n  \n c ")).toEqual(["a", "#b", "c"]);
    expect(expertLines("")).toEqual([]);
    expect(expertLines("x\n!x\nx")).toEqual(["x", "!x", "x"]);
  });

  it("isCommentLine is true for a line whose trimmed text starts with #", () => {
    expect(isCommentLine("  # x")).toBe(true);
    expect(isCommentLine("#")).toBe(true);
    expect(isCommentLine("*.log")).toBe(false);
    expect(isCommentLine("!#keep")).toBe(false);
    expect(isCommentLine("")).toBe(false);
  });
});

describe("excludesAfterModeSwitch", () => {
  it("fills the expert text from the list, comments included", () => {
    const f = valid({ excludeItems: ["# why", "x", "!x"] });
    expect(excludesAfterModeSwitch(f, "expert")).toEqual({ excludeMode: "expert", excludeText: "# why\nx\n!x" });
  });

  it("leaving expert keeps every line in order: comments and repeats, no de-duplication", () => {
    const f = valid({ excludeMode: "expert", excludeText: " x \n# why\n\n!x\nx\n# why" });
    expect(excludesAfterModeSwitch(f, "simple")).toEqual({
      excludeMode: "simple",
      excludeItems: ["x", "# why", "!x", "x", "# why"],
    });
  });
});

describe("withExcludeAdded", () => {
  it("appends a trimmed pattern, and ignores a blank one or a repeated pattern", () => {
    expect(withExcludeAdded(["a"], " b ")).toEqual(["a", "b"]);
    expect(withExcludeAdded(["a"], "   ")).toBeNull();
    expect(withExcludeAdded(["a"], "a")).toBeNull();
  });

  it("accepts a comment, and a comment already in the list again", () => {
    expect(withExcludeAdded(["a"], "# note")).toEqual(["a", "# note"]);
    expect(withExcludeAdded(["# note"], " # note ")).toEqual(["# note", "# note"]);
  });
});

describe("treeRulesFromForm", () => {
  it("feeds the preview the sources and exclusions as they'd be saved", () => {
    const f = valid({ excludeItems: ["a"], excludeIfPresent: [".x"], excludeCaches: true });
    expect(treeRulesFromForm(f)).toEqual({
      sources: ["/home/u/docs"],
      excludes: ["a"],
      excludeIfPresent: [".x"],
      excludeCaches: true,
      excludeFiles: [],
      filesFrom: [],
    });
    // Expert text goes in raw, comments and blanks included.
    expect(treeRulesFromForm(valid({ excludeMode: "expert", excludeText: "a\n# c\n\nb" })).excludes).toEqual(["a", "# c", "", "b"]);
  });
});

describe("tabs and Settings sections", () => {
  it("has two tabs and five Settings sections, in order, with their labels", () => {
    expect(PLAN_TABS).toEqual(["files", "settings"]);
    expect(PLAN_TAB_LABELS).toEqual({ files: "Files", settings: "Settings" });
    expect(SETTINGS_SECTIONS).toEqual(["retention", "tags", "bandwidth", "restic", "webhooks"]);
    expect(SETTINGS_SECTIONS.map((s) => SETTINGS_SECTION_LABELS[s])).toEqual([
      "Retention",
      "Tags",
      "Bandwidth",
      "Restic options",
      "Webhooks",
    ]);
  });

  it("recognises tab and section ids from the URL", () => {
    expect(isPlanTab("files")).toBe(true);
    expect(isPlanTab("settings")).toBe(true);
    // The old tabs are Settings sections now.
    expect(isPlanTab("tags")).toBe(false);
    expect(isPlanTab("advanced")).toBe(false);
    expect(isPlanTab("nope")).toBe(false);
    expect(isPlanTab(null)).toBe(false);
    for (const s of SETTINGS_SECTIONS) expect(isSettingsSection(s)).toBe(true);
    expect(isSettingsSection("advanced")).toBe(false);
    expect(isSettingsSection("files")).toBe(false);
    expect(isSettingsSection("")).toBe(false);
    expect(isSettingsSection(null)).toBe(false);
  });
});

describe("planLocation", () => {
  it("opens a Settings section named in the URL", () => {
    expect(planLocation("settings", "webhooks")).toEqual({ tab: "settings", section: "webhooks" });
    expect(planLocation("settings", "retention")).toEqual({ tab: "settings", section: "retention" });
    expect(planLocation("settings", null)).toEqual({ tab: "settings", section: null });
    expect(planLocation("files", null)).toEqual({ tab: "files", section: null });
  });

  it("opens Settings at the matching section for the old tab links", () => {
    expect(planLocation("retention", null)).toEqual({ tab: "settings", section: "retention" });
    expect(planLocation("tags", null)).toEqual({ tab: "settings", section: "tags" });
    expect(planLocation("advanced", null)).toEqual({ tab: "settings", section: "bandwidth" });
  });

  it("tolerates malformed links: an unknown section, a section without a tab, an unknown tab", () => {
    expect(planLocation("settings", "nope")).toEqual({ tab: "settings", section: null });
    expect(planLocation(null, "tags")).toEqual({ tab: "files", section: null });
    expect(planLocation("files", "tags")).toEqual({ tab: "files", section: null });
    expect(planLocation("zzz", null)).toEqual({ tab: "files", section: null });
    expect(planLocation("", "")).toEqual({ tab: "files", section: null });
    expect(planLocation(null, null)).toEqual({ tab: "files", section: null });
    // Not fooled by names every object has.
    expect(planLocation("constructor", null)).toEqual({ tab: "files", section: null });
    expect(planLocation("settings", "toString")).toEqual({ tab: "settings", section: null });
  });
});

describe("issueTab", () => {
  it("names the tab a problem is on: Files, Settings for any section, none for the header", () => {
    expect(issueTab("files")).toBe("files");
    for (const s of SETTINGS_SECTIONS) expect(issueTab(s)).toBe("settings");
    expect(issueTab("header")).toBeNull();
  });

  it("agrees with every place validatePlanForm reports", () => {
    const hook = { id: "w", url: "ftp://x", provider: "generic" as const, stages: { started: false, completed: true, failed: true } };
    const cases: [Partial<PlanFormState>, ReturnType<typeof issueTab>][] = [
      [{ name: "" }, null],
      [{ paths: [] }, "files"],
      [{ filesFrom: ["/l.txt"] }, "settings"],
      [{ webhooks: [hook] }, "settings"],
      [{ packSize: "1" }, "settings"],
      [{ retention: keep({ keepDaily: "0" }) }, "settings"],
    ];
    for (const [over, tab] of cases) {
      const problem = validatePlanForm(valid(over));
      expect(problem).not.toBeNull();
      expect(issueTab(problem!.where)).toBe(tab);
    }
  });
});

describe("sectionIsCustom", () => {
  const hook = { id: "w", url: "https://x", provider: "generic" as const, stages: { started: false, completed: true, failed: true } };

  it("is false for every section of a fresh form", () => {
    for (const s of SETTINGS_SECTIONS) expect(sectionIsCustom(valid(), s)).toBe(false);
    expect(settingsIsCustom(valid())).toBe(false);
  });

  it("Retention: any field set", () => {
    expect(sectionIsCustom(valid({ retention: keep({ keepDaily: "7" }) }), "retention")).toBe(true);
    expect(sectionIsCustom(valid({ retention: keep({ keepYearly: "1" }) }), "retention")).toBe(true);
    // Blank (whitespace) fields are unset.
    expect(sectionIsCustom(valid({ retention: keep({ keepDaily: "  " }) }), "retention")).toBe(false);
  });

  it("Tags: at least one tag, a typed-but-not-added one included (it is saved)", () => {
    expect(sectionIsCustom(valid({ tags: ["t"] }), "tags")).toBe(true);
    expect(sectionIsCustom(valid({ tagInput: " t " }), "tags")).toBe(true);
    expect(sectionIsCustom(valid({ tagInput: "   " }), "tags")).toBe(false);
  });

  it("Bandwidth: an upload or download limit set", () => {
    expect(sectionIsCustom(valid({ limitUpload: "1" }), "bandwidth")).toBe(true);
    expect(sectionIsCustom(valid({ limitDownload: "1" }), "bandwidth")).toBe(true);
    expect(sectionIsCustom(valid({ limitUpload: " " }), "bandwidth")).toBe(false);
  });

  it("Restic options: a pack size or a Windows-only flag", () => {
    expect(sectionIsCustom(valid({ packSize: "32" }), "restic")).toBe(true);
    expect(sectionIsCustom(valid({ excludeCloudFiles: true }), "restic")).toBe(true);
    expect(sectionIsCustom(valid({ useFsSnapshot: true }), "restic")).toBe(true);
    expect(sectionIsCustom(valid({ packSize: " " }), "restic")).toBe(false);
  });

  it("Webhooks: at least one", () => {
    expect(sectionIsCustom(valid({ webhooks: [hook] }), "webhooks")).toBe(true);
  });

  it("each setting dots only its own section", () => {
    const one: [Partial<PlanFormState>, string][] = [
      [{ retention: keep({ keepDaily: "7" }) }, "retention"],
      [{ tags: ["t"] }, "tags"],
      [{ limitUpload: "1" }, "bandwidth"],
      [{ packSize: "32" }, "restic"],
      [{ webhooks: [hook] }, "webhooks"],
    ];
    for (const [over, section] of one) {
      const f = valid(over);
      expect(SETTINGS_SECTIONS.filter((s) => sectionIsCustom(f, s))).toEqual([section]);
      expect(settingsIsCustom(f)).toBe(true);
    }
  });

  it("is not moved by the Files tab's settings", () => {
    const f = valid({ excludeItems: ["*.tmp"], excludeIfPresent: [".nobackup"], excludeCaches: true, filesFrom: ["/l.txt"] });
    expect(settingsIsCustom(f)).toBe(false);
  });
});
