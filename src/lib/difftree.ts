// Pure tree-building over flat DiffEntry[] output, shared with DiffPage.tsx's directory
// navigation. Moved out of the page (where they were module-private) so they're directly
// unit-testable — see difftree.test.ts.
import type { DiffEntry } from "./types";

export type DiffChange = "added" | "removed" | "modified" | "mixed";

export interface TreeNode {
  name: string;
  fullPath: string;
  isDir: boolean;
  change: DiffChange;
}

export function toSegments(path: string): string[] {
  return path.replace(/^\//, "").split("/").filter(Boolean);
}

/** Groups the flat `entries` list into the direct children of `currentPath` for DiffPage's
 *  directory browser. A child is a directory (`isDir: true`) when at least one grouped entry
 *  has segments deeper than it; a directory's `change` is `"mixed"` when its descendants don't
 *  all share the same change type, otherwise that one shared type. */
export function computeChildren(entries: DiffEntry[], currentPath: string): TreeNode[] {
  const currentSegments = currentPath ? currentPath.split("/").filter(Boolean) : [];
  const depth = currentSegments.length;

  type Parsed = { entry: DiffEntry; segs: string[] };
  const groups = new Map<string, Parsed[]>();

  for (const entry of entries) {
    const segs = toSegments(entry.path);
    if (segs.length <= depth) continue;
    if (!currentSegments.every((seg, i) => segs[i] === seg)) continue;
    const segment = segs[depth];
    if (!groups.has(segment)) groups.set(segment, []);
    groups.get(segment)!.push({ entry, segs });
  }

  const nodes: TreeNode[] = [];
  for (const [name, parsed] of groups) {
    const fullPath = currentPath + "/" + name;
    const nextDepth = depth + 1;
    const isDir = parsed.some((p) => p.segs.length > nextDepth);
    let change: DiffChange;
    if (!isDir) {
      change = parsed[0].entry.change as DiffChange;
    } else {
      const types = new Set(parsed.map((p) => p.entry.change));
      change = types.size === 1 ? ([...types][0] as DiffChange) : "mixed";
    }
    nodes.push({ name, fullPath, isDir, change });
  }

  return nodes.sort((a, b) => {
    if (a.isDir && !b.isDir) return -1;
    if (!a.isDir && b.isDir) return 1;
    return a.name.localeCompare(b.name);
  });
}
