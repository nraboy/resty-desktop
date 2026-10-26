import { describe, it, expect } from "vitest";
import { escapeGlob, scopeOptions, summarizeEdit } from "./planTreeEdit";

describe("escapeGlob", () => {
  it("escapes exactly the glob metacharacters", () => {
    expect(escapeGlob("/a/b c/d.txt")).toBe("/a/b c/d.txt");
    expect(escapeGlob("/a/we[ird]/x*y?")).toBe("/a/we\\[ird\\]/x\\*y\\?");
    expect(escapeGlob("back\\slash")).toBe("back\\\\slash");
  });
});

describe("scopeOptions", () => {
  const sources = ["/home/u/code"];

  it("offers exact, contents, name-anywhere and nearer-ancestor scopes for a folder", () => {
    const opts = scopeOptions("/home/u/code/app/web/node_modules", "dir", sources);
    expect(opts.map((o) => o.id)).toEqual(["exact", "contents", "name", "under:/home/u/code/app/web", "under:/home/u/code/app", "under:/home/u/code"]);
    expect(opts[0].pattern).toBe("/home/u/code/app/web/node_modules");
    expect(opts[1].pattern).toBe("/home/u/code/app/web/node_modules/*");
    expect(opts[2].pattern).toBe("node_modules");
    expect(opts[3].pattern).toBe("/home/u/code/app/web/**/node_modules");
  });

  it("never scopes above the source", () => {
    const ids = scopeOptions("/home/u/code/app/x.log", "file", sources).map((o) => o.id);
    expect(ids).not.toContain("under:/home/u");
    expect(ids).toEqual(["exact", "name", "under:/home/u/code/app", "under:/home/u/code", "ext"]);
  });

  it("offers an extension only for files that have one", () => {
    expect(scopeOptions("/s/a/archive.tar.gz", "file", ["/s"]).find((o) => o.id === "ext")?.pattern).toBe("*.gz");
    expect(scopeOptions("/s/a/Makefile", "file", ["/s"]).some((o) => o.id === "ext")).toBe(false);
    expect(scopeOptions("/s/a/.env", "file", ["/s"]).some((o) => o.id === "ext")).toBe(false);
    expect(scopeOptions("/s/a/dir.d", "dir", ["/s"]).some((o) => o.id === "ext")).toBe(false);
  });

  it("escapes glob characters in the path and the name", () => {
    const opts = scopeOptions("/s/we[ird]/a*b", "file", ["/s"]);
    expect(opts[0].pattern).toBe("/s/we\\[ird\\]/a\\*b");
    expect(opts.find((o) => o.id === "name")?.pattern).toBe("a\\*b");
    expect(opts.find((o) => o.id.startsWith("under:/s/we"))?.pattern).toBe("/s/we\\[ird\\]/**/a\\*b");
  });

  it("uses the deepest source containing the path", () => {
    const ids = scopeOptions("/s/inner/deep/x", "file", ["/s", "/s/inner"]).map((o) => o.id);
    expect(ids).toEqual(["exact", "name", "under:/s/inner/deep", "under:/s/inner"]);
  });
});

describe("summarizeEdit", () => {
  it("describes what changed", () => {
    const base = { sources: [], excludes: [], addedRules: [], addedSources: [], removedSources: [] };
    expect(summarizeEdit(base)).toBe("no change");
    expect(summarizeEdit({ ...base, addedRules: ["x"] })).toBe("added pattern");
    expect(summarizeEdit({ ...base, addedRules: ["a", "b", "c"], removedSources: ["/t"] })).toBe("removed source /t; added 3 patterns");
    expect(summarizeEdit({ ...base, addedSources: ["/n"] })).toBe("added source /n");
  });
});

describe("Windows", () => {
  it("escapes a bracket as a class, since a backslash is a separator there", () => {
    expect(escapeGlob("C:/a/we[ird]", true)).toBe("C:/a/we[[]ird]");
    expect(escapeGlob("C:/a/b c", true)).toBe("C:/a/b c");
  });

  it("offers the same scopes, rooted at the drive", () => {
    const opts = scopeOptions("C:/Users/me/code/app/node_modules", "dir", ["C:/Users/me/code"], true);
    expect(opts.map((o) => o.id)).toEqual(["exact", "contents", "name", "under:C:/Users/me/code/app", "under:C:/Users/me/code"]);
    expect(opts[0].pattern).toBe("C:/Users/me/code/app/node_modules");
    expect(opts[3].pattern).toBe("C:/Users/me/code/app/**/node_modules");
    // A drive-root source works too; like `/` on Unix, the root itself isn't offered as a scope
    // (it would say the same as "named x.log anywhere").
    const d = scopeOptions("D:/data/x.log", "file", ["D:/"], true).map((o) => o.id);
    expect(d).toEqual(["exact", "name", "under:D:/data", "ext"]);
  });
});
