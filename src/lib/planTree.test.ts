import { describe, it, expect } from "vitest";
import {
  ancestorsOf,
  baseName,
  isInside,
  isRootPath,
  joinPath,
  rootOf,
  toCanonical,
  toNative,
  displayState,
  flattenRows,
  hiddenNoteText,
  hiddenSummary,
  leftClick,
  leftClickLabel,
  menuEntries,
  pinnedPaths,
  reasonLines,
  relativeTo,
  topLevelSources,
  visibleWindow,
  type DisplayState,
  type NodeInfo,
  type NodeVerdict,
  type SourceKind,
  type RuleOrigin,
  type ScanResult,
} from "./planTree";

const verdict = (over: Partial<NodeVerdict> = {}): NodeVerdict => ({
  path: "/s/a",
  included: true,
  containsSources: false,
  ...over,
});
const scan = (over: Partial<ScanResult> = {}): ScanResult => ({
  partial: false,
  complete: true,
  unreadable: false,
  scanned: 10,
  ...over,
});

describe("displayState", () => {
  it("is pending until there is a verdict", () => {
    expect(displayState("dir", undefined, undefined)).toBe("pending");
  });

  it("maps verdicts and scans to the three states plus the in-between ones", () => {
    expect(displayState("dir", verdict({ included: false }), undefined)).toBe("excluded");
    expect(displayState("dir", verdict({ included: false, containsSources: true }), undefined)).toBe("contains");
    expect(displayState("file", verdict(), undefined)).toBe("included");
    expect(displayState("dir", verdict(), undefined)).toBe("pending");
    expect(displayState("dir", verdict(), scan({ partial: true }))).toBe("partial");
    expect(displayState("dir", verdict(), scan())).toBe("included");
    expect(displayState("dir", verdict(), scan({ complete: false }))).toBe("unverified");
  });
});

describe("reasonLines", () => {
  const origins = new Map<number, RuleOrigin>([
    [1, { id: 1, pattern: "node_modules" }],
    [2, { id: 2, pattern: "*.log", file: "/home/u/.ignore", line: 7 }],
    [3, { id: 3, pattern: "!/s/a" }],
  ]);

  it("names the pattern, and the ancestor when the exclusion is inherited", () => {
    const own = verdict({ included: false, reason: { kind: "pattern", ruleId: 1 } });
    expect(reasonLines("excluded", "/s/node_modules", own, undefined, origins)).toEqual(["Excluded by `node_modules`."]);

    const inherited = verdict({ included: false, reason: { kind: "pattern", ruleId: 1 }, at: "/s/node_modules" });
    expect(reasonLines("excluded", "/s/node_modules/x.js", inherited, undefined, origins, "file")[0]).toBe(
      "Excluded because its folder /s/node_modules is excluded by `node_modules`.",
    );
  });

  it("points at the file and line for a rule that came from an exclude file", () => {
    const v = verdict({ included: false, reason: { kind: "pattern", ruleId: 2 } });
    expect(reasonLines("excluded", "/s/x.log", v, undefined, origins, "file")[0]).toBe(
      "Excluded by `*.log` (line 7 of /home/u/.ignore).",
    );
  });

  it("explains markers, outside-source paths, partial folders and re-includes", () => {
    const marker = verdict({ included: false, reason: { kind: "marker", spec: ".nobackup" } });
    expect(reasonLines("excluded", "/s/b/x", marker, undefined, origins, "file")[0]).toContain("`.nobackup`");

    const outside = verdict({ included: false, reason: { kind: "outsideSources" } });
    expect(reasonLines("excluded", "/etc", outside, undefined, origins)).toEqual(["Not inside any backup source."]);

    const partial = reasonLines("partial", "/s", verdict(), scan({ partial: true, firstExcluded: "/s/a/node_modules" }), origins);
    expect(partial).toEqual(["Backed up, but some items inside are excluded.", "For example: a/node_modules"]);

    const re = reasonLines("included", "/s/a", verdict({ reincludedBy: 3 }), scan(), origins);
    expect(re).toContain("Re-included by `!/s/a`.");
  });

  it("mentions unreadable folders and missing paths", () => {
    expect(reasonLines("included", "/s", verdict(), scan({ unreadable: true }), origins).join(" ")).toContain("Full Disk Access");
    expect(reasonLines("excluded", "/gone", undefined, undefined, origins, "missing")).toEqual(["This path doesn't exist."]);
  });
});

describe("flattenRows", () => {
  const dir = (path: string, children?: string[], extra: Partial<NodeInfo> = {}): NodeInfo => ({
    path,
    name: path.split("/").pop() || "/",
    kind: "dir",
    children,
    ...extra,
  });
  const file = (path: string): NodeInfo => ({ path, name: path.split("/").pop()!, kind: "file" });

  it("shows children only for expanded, listed folders, with depth", () => {
    const nodes = new Map<string, NodeInfo>([
      ["/s", dir("/s", ["/s/a", "/s/f.txt"])],
      ["/s/a", dir("/s/a", ["/s/a/x"])],
      ["/s/a/x", file("/s/a/x")],
      ["/s/f.txt", file("/s/f.txt")],
    ]);
    const closed = flattenRows([{ path: "/s" }], nodes, new Set());
    expect(closed.map((r) => r.type === "node" && r.path)).toEqual(["/s"]);

    const open = flattenRows([{ path: "/s" }], nodes, new Set(["/s", "/s/a"]));
    expect(open.map((r) => (r.type === "node" ? `${r.depth}:${r.path}` : r.type))).toEqual([
      "0:/s",
      "1:/s/a",
      "2:/s/a/x",
      "1:/s/f.txt",
    ]);
  });

  it("adds notes for read errors, truncation and empty folders", () => {
    const nodes = new Map<string, NodeInfo>([
      ["/e", dir("/e", undefined, { error: "Permission denied" })],
      ["/t", dir("/t", ["/t/a"], { truncated: true, total: 5000 })],
      ["/t/a", { path: "/t/a", name: "a", kind: "file" }],
      ["/z", dir("/z", [])],
    ]);
    const rows = flattenRows([{ path: "/e" }, { path: "/t" }, { path: "/z" }], nodes, new Set(["/e", "/t", "/z"]));
    const notes = rows.filter((r) => r.type === "note").map((r) => r.type === "note" && r.text);
    expect(notes).toEqual(["Can't read this folder: Permission denied", "…and 4,999 more not shown", "Empty folder"]);
  });

  it("uses a root's display name when given", () => {
    const nodes = new Map<string, NodeInfo>([["/s", dir("/s", [])]]);
    const rows = flattenRows([{ path: "/s", name: "/very/long/source/path" }], nodes, new Set());
    expect(rows[0].type === "node" && rows[0].name).toBe("/very/long/source/path");
  });
});

describe("path helpers", () => {
  it("relativeTo", () => {
    expect(relativeTo("/s/a/b", "/s")).toBe("a/b");
    expect(relativeTo("/a/b", "/")).toBe("a/b");
    expect(relativeTo("/other", "/s")).toBe("/other");
  });

  it("ancestorsOf lists every folder above a path", () => {
    expect(ancestorsOf("/home/u/code")).toEqual(["/", "/home", "/home/u"]);
    expect(ancestorsOf("/x")).toEqual(["/"]);
  });

  it("topLevelSources drops sources nested inside another and duplicates", () => {
    expect(topLevelSources(["/a", "/a/b", "/c", "/c", " /d ", "/ab"])).toEqual(["/a", "/c", "/d", "/ab"]);
    expect(topLevelSources(["/", "/a"])).toEqual(["/"]);
  });
});

describe("visibleWindow", () => {
  it("renders the viewport plus overscan, clamped to the row count", () => {
    expect(visibleWindow(1000, 0, 260, 26, 6)).toEqual({ start: 0, end: 16 });
    expect(visibleWindow(1000, 2600, 260, 26, 6)).toEqual({ start: 94, end: 116 });
    expect(visibleWindow(10, 0, 260, 26, 6)).toEqual({ start: 0, end: 10 });
    expect(visibleWindow(0, 0, 260, 26)).toEqual({ start: 0, end: 0 });
  });
});

describe("Windows paths", () => {
  it("recognises roots — `/` and drives", () => {
    expect(rootOf("/home/u")).toBe("/");
    expect(rootOf("C:/Users/me")).toBe("C:/");
    expect(rootOf("c:/")).toBe("c:/");
    expect(rootOf("relative/x")).toBeNull();
    expect(isRootPath("C:/")).toBe(true);
    expect(isRootPath("C:/Users")).toBe(false);
    expect(isRootPath("/")).toBe(true);
  });

  it("joins under a root without doubling the separator", () => {
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("C:/", "Users")).toBe("C:/Users");
    expect(joinPath("C:/Users", "me")).toBe("C:/Users/me");
  });

  it("converts between the OS spelling and the tree's, only on Windows", () => {
    expect(toCanonical("C:\\Users\\me\\", true)).toBe("C:/Users/me");
    expect(toCanonical("C:\\", true)).toBe("C:/");
    expect(toCanonical("  D:\\data  ", true)).toBe("D:/data");
    // A backslash is an ordinary character in a Unix file name.
    expect(toCanonical("/home/u/we\\ird/", false)).toBe("/home/u/we\\ird");
    expect(toNative("C:/Users/me", true)).toBe("C:\\Users\\me");
    expect(toNative("/home/u", false)).toBe("/home/u");
  });

  it("lists ancestors, nesting and relative paths per drive", () => {
    expect(ancestorsOf("C:/Users/me/code")).toEqual(["C:/", "C:/Users", "C:/Users/me"]);
    expect(ancestorsOf("D:/x")).toEqual(["D:/"]);
    expect(ancestorsOf("relative")).toEqual([]);
    expect(isInside("C:/Users/me", "C:/Users")).toBe(true);
    expect(isInside("C:/Users", "C:/Users")).toBe(false);
    expect(isInside("C:/Usersx", "C:/Users")).toBe(false);
    expect(isInside("C:/Users", "C:/")).toBe(true);
    expect(isInside("D:/Users", "C:/")).toBe(false);
    expect(relativeTo("C:/Users/me/a", "C:/Users")).toBe("me/a");
    expect(relativeTo("C:/a", "C:/")).toBe("a");
    expect(baseName("C:/Users/me")).toBe("me");
    expect(baseName("C:/")).toBe("C:");
    expect(topLevelSources(["C:/", "C:/Users", "D:/data", "D:/data/x"])).toEqual(["C:/", "D:/data"]);
  });
});

describe("hidden files", () => {
  const nodes = () =>
    new Map<string, NodeInfo>([
      ["/h", { path: "/h", name: "h", kind: "dir", children: ["/h/.cache", "/h/.rc", "/h/docs", "/h/.config"] }],
      ["/h/.cache", { path: "/h/.cache", name: ".cache", kind: "dir", hidden: true }],
      ["/h/.rc", { path: "/h/.rc", name: ".rc", kind: "file", hidden: true }],
      ["/h/docs", { path: "/h/docs", name: "docs", kind: "dir" }],
      ["/h/.config", { path: "/h/.config", name: ".config", kind: "dir", hidden: true, children: ["/h/.config/app"] }],
      ["/h/.config/app", { path: "/h/.config/app", name: "app", kind: "dir" }],
    ]);
  const included = (p: string) => p !== "/h/.cache";
  const names = (rows: ReturnType<typeof flattenRows>) =>
    rows.map((r) => (r.type === "node" ? r.name : `[${r.text}]`));

  it("shows everything when hidden items are on, or when no option is given", () => {
    const open = new Set(["/h"]);
    expect(names(flattenRows([{ path: "/h" }], nodes(), open))).toEqual(["h", ".cache", ".rc", "docs", ".config"]);
    expect(
      names(flattenRows([{ path: "/h" }], nodes(), open, { showHidden: true, pinned: new Set() })),
    ).toEqual(["h", ".cache", ".rc", "docs", ".config"]);
  });

  it("leaves hidden items out when off, with a note saying how many are backed up", () => {
    const rows = flattenRows([{ path: "/h" }], nodes(), new Set(["/h"]), {
      showHidden: false,
      pinned: new Set(),
      included,
    });
    expect(names(rows)).toEqual(["h", "docs", "[3 hidden items not shown — 2 are backed up]"]);
    const note = rows[rows.length - 1];
    expect(note.type === "note" && note.action).toBe("showHidden");
  });

  it("keeps a hidden source, or a hidden folder on the way to one, visible", () => {
    const pinned = pinnedPaths(["/h/.config/app"]);
    const rows = flattenRows([{ path: "/h" }], nodes(), new Set(["/h", "/h/.config"]), {
      showHidden: false,
      pinned,
      included,
    });
    expect(names(rows)).toEqual(["h", "docs", ".config", "app", "[2 hidden items not shown — 1 is backed up]"]);
    // Sorted listing order is kept: the pinned folder sits where it was, the others are summarised after.
  });

  it("words the note for none / one / all being backed up", () => {
    expect(hiddenNoteText(2, 0)).toBe("2 hidden items not shown — none are backed up");
    expect(hiddenNoteText(1, 1)).toBe("1 hidden item not shown — it is backed up");
    expect(hiddenNoteText(4, 4)).toBe("4 hidden items not shown — all are backed up");
    expect(hiddenNoteText(1200, 3)).toBe("1,200 hidden items not shown — 3 are backed up");
  });

  it("counts hidden items and the backed-up ones across everything listed", () => {
    expect(hiddenSummary(nodes(), included)).toEqual({ hidden: 3, backedUp: 2 });
    expect(hiddenSummary(new Map(), included)).toEqual({ hidden: 0, backedUp: 0 });
  });

  it("pins a source and every folder above it", () => {
    expect([...pinnedPaths(["/a/b/c", "C:/x"])].sort()).toEqual(["/", "/a", "/a/b", "/a/b/c", "C:/", "C:/x"]);
  });
});

describe("leftClick", () => {
  const outside = verdict({ included: false, reason: { kind: "outsideSources" } });
  const excludedInside = verdict({ included: false, reason: { kind: "pattern", ruleId: 1 }, at: "/s/node_modules" });

  it("removes a source in one click", () => {
    expect(leftClick(true, verdict())).toBe("removeSource");
    // Whatever the verdict says, a source row is a source row.
    expect(leftClick(true, undefined)).toBe("removeSource");
  });

  it("adds an item outside every source, including a folder above one", () => {
    expect(leftClick(false, outside)).toBe("addSource");
    expect(leftClick(false, verdict({ included: false, containsSources: true, reason: { kind: "outsideSources" } }))).toBe("addSource");
  });

  it("includes an excluded item inside a source, whatever excluded it", () => {
    expect(leftClick(false, excludedInside)).toBe("include");
    expect(leftClick(false, verdict({ included: false, reason: { kind: "marker", spec: "x" } }))).toBe("include");
    expect(leftClick(false, verdict({ included: false }))).toBe("include");
  });

  it("excludes an included item, and an item whose verdict has not arrived", () => {
    expect(leftClick(false, verdict())).toBe("exclude");
    expect(leftClick(false, undefined)).toBe("exclude");
  });
});

describe("leftClickLabel", () => {
  it("words each click for the box tooltip, naming a folder or a file", () => {
    expect(leftClickLabel("exclude", "dir")).toBe("Exclude");
    expect(leftClickLabel("include", "file")).toBe("Include");
    expect(leftClickLabel("removeSource", "dir")).toBe("Stop backing up this folder");
    expect(leftClickLabel("addSource", "file")).toBe("Back up this file");
  });
});

describe("menuEntries", () => {
  const outside = verdict({ included: false, reason: { kind: "outsideSources" } });
  const excluded = verdict({ included: false, reason: { kind: "pattern", ruleId: 1 }, at: "/s/node_modules" });
  type Args = Parameters<typeof menuEntries>[0];
  const entries = (over: Partial<Args> = {}) =>
    menuEntries({ isSource: false, verdict: verdict(), state: "included", kind: "dir", exactRule: false, ...over });
  const actions = (over: Partial<Args> = {}) => entries(over).map((e) => e.action);

  it("offers Exclude and Exclude by pattern for an included item", () => {
    expect(actions()).toEqual(["exclude", "excludeByPattern"]);
    expect(actions({ kind: "file" })).toEqual(["exclude", "excludeByPattern"]);
    expect(entries().map((e) => e.label)).toEqual(["Exclude", "Exclude by pattern…"]);
  });

  it("offers them while the verdict or the folder scan is still pending", () => {
    expect(actions({ verdict: undefined, state: "pending" })).toEqual(["exclude", "excludeByPattern"]);
    expect(actions({ state: "pending" })).toEqual(["exclude", "excludeByPattern"]);
    expect(actions({ state: "unverified" })).toEqual(["exclude", "excludeByPattern"]);
  });

  it("adds Include everything inside it for a partly included folder", () => {
    expect(actions({ state: "partial" })).toEqual(["exclude", "excludeByPattern", "includeEverything"]);
    expect(entries({ state: "partial" })[2].label).toBe("Include everything inside it");
    // A folder with nothing excluded inside is not offered it.
    expect(actions({ state: "included" })).not.toContain("includeEverything");
  });

  it("offers just Include for an excluded file, even if its path has an exact rule", () => {
    expect(actions({ verdict: excluded, state: "excluded", kind: "file" })).toEqual(["include"]);
    expect(actions({ verdict: excluded, state: "excluded", kind: "file", exactRule: true })).toEqual(["include"]);
    expect(entries({ verdict: excluded, state: "excluded", kind: "file" })[0].label).toBe("Include");
  });

  it("offers just Include for an excluded folder with no exact rule of its own", () => {
    expect(actions({ verdict: excluded, state: "excluded" })).toEqual(["include"]);
  });

  it("adds the include-everything variant for an excluded folder with an exact rule", () => {
    const e = entries({ verdict: excluded, state: "excluded", exactRule: true });
    expect(e.map((x) => x.action)).toEqual(["include", "includeEverything"]);
    expect(e[1].label).toBe("Include, plus everything inside it that other rules exclude");
  });

  it("offers the one source action for a source, and for an item outside every source", () => {
    expect(entries({ isSource: true })).toEqual([{ action: "removeSource", label: "Stop backing up this folder" }]);
    expect(entries({ isSource: true, kind: "file" })).toEqual([{ action: "removeSource", label: "Stop backing up this file" }]);
    expect(entries({ verdict: outside, state: "excluded" })).toEqual([{ action: "addSource", label: "Back up this folder" }]);
    expect(entries({ verdict: outside, state: "excluded", kind: "file" })).toEqual([
      { action: "addSource", label: "Back up this file" },
    ]);
    // A folder above a source: not backed up itself, but adding it is still the single action.
    const above = verdict({ included: false, containsSources: true, reason: { kind: "outsideSources" } });
    expect(actions({ verdict: above, state: "contains" })).toEqual(["addSource"]);
    // The source options ignore the exact-rule and partial hints.
    expect(actions({ isSource: true, state: "partial", exactRule: true })).toEqual(["removeSource"]);
  });

  it("makes the first entry the left-click, so a right-click is never at odds with it", () => {
    const verdicts = [undefined, verdict(), excluded, outside, verdict({ included: false, containsSources: true, reason: { kind: "outsideSources" } })];
    const states: DisplayState[] = ["included", "partial", "excluded", "contains", "unverified", "pending"];
    const kinds: SourceKind[] = ["dir", "file", "symlink", "other", "missing"];
    for (const isSource of [true, false]) {
      for (const v of verdicts) {
        for (const state of states) {
          for (const kind of kinds) {
            for (const exactRule of [true, false]) {
              const list = menuEntries({ isSource, verdict: v, state, kind, exactRule });
              expect(list.length).toBeGreaterThan(0);
              expect(list[0].action).toBe(leftClick(isSource, v));
            }
          }
        }
      }
    }
  });
});
