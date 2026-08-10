import { describe, it, expect } from "vitest";
import { computeChildren, toSegments } from "./difftree";
import type { DiffEntry } from "./types";

function entry(path: string, change: DiffEntry["change"]): DiffEntry {
  return { path, change };
}

describe("toSegments", () => {
  it("splits a path into segments, dropping a leading slash", () => {
    expect(toSegments("/a/b/c")).toEqual(["a", "b", "c"]);
  });

  it("filters out empty segments (e.g. a doubled slash)", () => {
    expect(toSegments("/a//b")).toEqual(["a", "b"]);
  });

  it("handles a bare filename with no directory", () => {
    expect(toSegments("file.txt")).toEqual(["file.txt"]);
  });
});

describe("computeChildren", () => {
  it("groups root-level entries by their first path segment", () => {
    const entries = [entry("/a.txt", "added"), entry("/b.txt", "removed")];
    const children = computeChildren(entries, "");
    expect(children.map((c) => c.name)).toEqual(["a.txt", "b.txt"]);
    expect(children.every((c) => !c.isDir)).toBe(true);
  });

  it("filters to only the direct descendants of currentPath", () => {
    const entries = [
      entry("/dir/a.txt", "added"),
      entry("/other/b.txt", "removed"),
    ];
    const children = computeChildren(entries, "/dir");
    expect(children.map((c) => c.name)).toEqual(["a.txt"]);
  });

  it("detects a directory when a grouped entry has segments deeper than the next depth", () => {
    const entries = [entry("/dir/nested/file.txt", "added")];
    const children = computeChildren(entries, "");
    expect(children).toEqual([
      { name: "dir", fullPath: "/dir", isDir: true, change: "added" },
    ]);
  });

  it("detects a file when no grouped entry goes deeper", () => {
    const entries = [entry("/file.txt", "modified")];
    const children = computeChildren(entries, "");
    expect(children).toEqual([
      { name: "file.txt", fullPath: "/file.txt", isDir: false, change: "modified" },
    ]);
  });

  it("rolls a directory's descendants up to a single shared change type", () => {
    const entries = [
      entry("/dir/a.txt", "added"),
      entry("/dir/b.txt", "added"),
    ];
    const children = computeChildren(entries, "");
    expect(children[0]).toMatchObject({ name: "dir", isDir: true, change: "added" });
  });

  it("rolls a directory up to 'mixed' when its descendants have differing change types", () => {
    const entries = [
      entry("/dir/a.txt", "added"),
      entry("/dir/b.txt", "removed"),
    ];
    const children = computeChildren(entries, "");
    expect(children[0]).toMatchObject({ name: "dir", isDir: true, change: "mixed" });
  });

  it("sorts directories before files, then alphabetically within each group", () => {
    const entries = [
      entry("/zfile.txt", "added"),
      entry("/afile.txt", "added"),
      entry("/zdir/nested.txt", "added"),
      entry("/adir/nested.txt", "added"),
    ];
    const children = computeChildren(entries, "");
    expect(children.map((c) => c.name)).toEqual(["adir", "zdir", "afile.txt", "zfile.txt"]);
  });

  it("returns an empty array when nothing matches currentPath", () => {
    const entries = [entry("/dir/a.txt", "added")];
    expect(computeChildren(entries, "/nope")).toEqual([]);
  });
});
