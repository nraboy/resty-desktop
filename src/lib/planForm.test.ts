import { describe, it, expect } from "vitest";
import {
  advancedIsCustom,
  buildPlan,
  effectiveTags,
  emptyForm,
  expertPatterns,
  formFromPlan,
  hasRetention,
  isPlanTab,
  retentionSummary,
  savedExcludes,
  treeRulesFromForm,
  validatePlanForm,
  type PlanFormState,
} from "./planForm";
import type { BackupPlan } from "./types";

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

  it("rejects a webhook that isn't http(s), and a bad pack size, on the Advanced tab", () => {
    const hook = (url: string) => ({ id: "w", url, provider: "generic" as const, stages: { started: false, completed: true, failed: true } });
    expect(validatePlanForm(valid({ webhooks: [hook("ftp://x")] }))).toEqual({
      where: "advanced",
      message: 'Webhook "ftp://x" must start with http:// or https://.',
    });
    expect(validatePlanForm(valid({ webhooks: [hook(" https://ok ")] }))).toBeNull();
    const pack = validatePlanForm(valid({ packSize: "3" }));
    expect(pack?.where).toBe("advanced");
    expect(pack?.message).toContain("between");
    expect(validatePlanForm(valid({ packSize: "64" }))).toBeNull();
  });

  it("rejects retention counts below 1 or non-numeric on the Retention tab", () => {
    for (const bad of ["0", "-2", "abc"]) {
      expect(validatePlanForm(valid({ keepDaily: bad }))).toEqual({
        where: "retention",
        message: "Retention counts must be whole numbers of at least 1 — leave a field blank to disable it.",
      });
    }
    expect(validatePlanForm(valid({ keepDaily: "7", keepWeekly: "0" }))?.where).toBe("retention");
    expect(validatePlanForm(valid({ keepDaily: "7" }))).toBeNull();
    expect(validatePlanForm(valid({ keepDaily: "  " }))).toBeNull();
  });

  it("reports the first problem when several tabs have one", () => {
    expect(validatePlanForm(valid({ packSize: "1", keepDaily: "0" }))?.where).toBe("advanced");
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
        keepDaily: "7",
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

  it("saves expert text as trimmed patterns without comments or blanks, in order", () => {
    const f = valid({ excludeMode: "expert", excludeText: "  node_modules \n# note\n\n!keep\n*.log" });
    expect(buildPlan(f, "i").excludes).toEqual(["node_modules", "!keep", "*.log"]);
    expect(savedExcludes(f)).toEqual(["node_modules", "!keep", "*.log"]);
    expect(expertPatterns("a\n#b\n  \n c ")).toEqual(["a", "c"]);
    // Simple mode saves the list as is, duplicates and order included (order is meaning).
    expect(savedExcludes(valid({ excludeItems: ["!x", "y", "!x"] }))).toEqual(["!x", "y", "!x"]);
  });

  it("includes a typed-but-not-added tag, once", () => {
    expect(effectiveTags(valid({ tags: ["a"], tagInput: " b " }))).toEqual(["a", "b"]);
    expect(effectiveTags(valid({ tags: ["a"], tagInput: "a" }))).toEqual(["a"]);
    expect(effectiveTags(valid({ tags: ["a"], tagInput: "  " }))).toEqual(["a"]);
    expect(buildPlan(valid({ tagInput: "t" }), "i").tags).toEqual(["t"]);
  });

  it("has no retention when every count is blank, and parses the ones set", () => {
    expect(buildPlan(valid(), "i").retention).toBeUndefined();
    expect(hasRetention(valid({ keepYearly: "2" }))).toBe(true);
    expect(buildPlan(valid({ keepLast: "3", keepYearly: "2" }), "i").retention).toEqual({
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
    expect(f.keepDaily).toBe("7");
    expect(f.packSize).toBe("32");
    expect(f.limitDownload).toBe("");
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

describe("tab summaries", () => {
  it("summarises retention", () => {
    expect(retentionSummary(valid())).toBe("Off");
    expect(retentionSummary(valid({ keepDaily: "7", keepWeekly: "4" }))).toBe("Keep 7 daily, 4 weekly");
    expect(retentionSummary(valid({ keepLast: " 3 ", keepYearly: "1" }))).toBe("Keep 3 last, 1 yearly");
  });

  it("flags anything non-default on the Advanced tab", () => {
    expect(advancedIsCustom(valid())).toBe(false);
    expect(advancedIsCustom(valid({ packSize: "32" }))).toBe(true);
    expect(advancedIsCustom(valid({ limitUpload: "1" }))).toBe(true);
    expect(advancedIsCustom(valid({ limitDownload: "1" }))).toBe(true);
    expect(advancedIsCustom(valid({ excludeCloudFiles: true }))).toBe(true);
    expect(advancedIsCustom(valid({ useFsSnapshot: true }))).toBe(true);
    expect(advancedIsCustom(valid({ webhooks: [{ id: "w", url: "https://x", provider: "generic", stages: { started: false, completed: true, failed: true } }] }))).toBe(true);
    // Retention and tags have their own tabs.
    expect(advancedIsCustom(valid({ keepDaily: "7", tags: ["t"] }))).toBe(false);
  });

  it("recognises tab ids from the URL", () => {
    expect(isPlanTab("files")).toBe(true);
    expect(isPlanTab("tags")).toBe(true);
    expect(isPlanTab("nope")).toBe(false);
    expect(isPlanTab(null)).toBe(false);
  });
});
