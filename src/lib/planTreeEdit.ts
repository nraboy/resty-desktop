// Pure helpers for editing a plan from the tree (components/PlanTreeEditDialog.tsx): the
// pattern choices offered for a clicked path. The edits themselves are synthesised and verified
// by the Rust engine (commands/plan_edit.rs); the patterns offered here are plain restic
// patterns and are only ever applied through that verification.

import { baseName, isInside, joinPath, rootOf, type MenuAction, type SourceKind } from "./planTree";

/** The result of `tree_plan_edit`: the plan's sources and patterns after the edit. */
export interface EditOutcome {
  sources: string[];
  excludes: string[];
  addedRules: string[];
  /** Real patterns the edit deleted (an add-source edit can return these with no `addedRules`).
   * Neither list ever holds the `# Tree: …` comment lines that `excludes` may. */
  removedRules: string[];
  addedSources: string[];
  removedSources: string[];
  /** Items inside a re-included folder that other rules had excluded and that came back with it.
   * Absent or null when nothing did. */
  broughtBack?: { count: number; capped: boolean } | null;
}

/** `exclude` adds `pattern`; `excludeItem` excludes exactly `path`; `include` re-includes `path`;
 * `includeEverything` also brings back what other rules exclude inside it. */
export interface EditAction {
  kind: "exclude" | "excludeItem" | "include" | "includeEverything";
  path: string;
  pattern?: string;
}

/** The edit the backend should compute for a menu action; null for "Exclude by pattern…", which
 * is a dialog (the pattern is chosen there) rather than an edit. Taking a source away is an
 * exclude of its exact path, as the dialog's old "remove" did. */
export function editActionFor(action: MenuAction, path: string, windows: boolean): EditAction | null {
  switch (action) {
    case "exclude":
      return { kind: "excludeItem", path };
    case "include":
    case "addSource":
      return { kind: "include", path };
    case "includeEverything":
      return { kind: "includeEverything", path };
    case "removeSource":
      return { kind: "exclude", path, pattern: escapeGlob(path, windows) };
    case "excludeByPattern":
      return null;
  }
}

/** Should an add-source edit be shown to the user before it is applied? It should whenever it
 * would swallow existing sources or change the exclude list — add patterns or delete some (it can
 * delete without adding) — so a click never changes rules unseen. */
export function needsConfirmation(o: EditOutcome): boolean {
  return o.removedSources.length > 0 || o.addedRules.length > 0 || o.removedRules.length > 0;
}

export interface MatchPreview {
  count: number;
  sample: string[];
  capped: boolean;
}

export interface ExportOut {
  excludeFileName: string;
  excludeFile: string;
  iexcludeFileName?: string | null;
  iexcludeFile?: string | null;
  command: string;
  notes: string[];
}

/** Escapes the characters Go's `filepath.Match` (and so restic) treats specially, so a literal
 * path or name can be used inside a pattern. Mirrors `escape_glob` in plan_edit.rs. */
export function escapeGlob(s: string, windows = false): string {
  // On Windows a backslash is a path separator, not an escape, so a literal `[` is written as the
  // class `[[]` (`*` and `?` can't occur in a Windows file name).
  if (windows) return s.replace(/\[/g, "[[]");
  return s.replace(/[\\[\]*?]/g, (c) => `\\${c}`);
}

/** Does the plan's own pattern list hold a line that is exactly this path — what a click on the
 * path would have written? A re-include (`!`) or a comment (`#`) line never counts. Mirrors the
 * engine's notion of an exact rule (plan_edit.rs). */
export function hasExactRule(excludes: string[], path: string, windows: boolean): boolean {
  const exact = escapeGlob(path, windows);
  return excludes.some((line) => {
    const t = line.trim();
    return t === exact && !t.startsWith("!") && !t.startsWith("#");
  });
}

export interface ScopeOption {
  id: string;
  label: string;
  /** The restic pattern this choice adds. */
  pattern: string;
}

/** The folders between a source and the path's own folder, nearest first (at most `max`). */
function ancestorsBelowSource(path: string, sources: string[], max: number): string[] {
  const src = sources
    .filter((s) => path === s || isInside(path, s))
    .sort((a, b) => b.length - a.length)[0];
  const root = rootOf(path);
  if (!root) return [];
  const parts = path.slice(root.length).split("/").filter(Boolean);
  const chain: string[] = [];
  let acc = root;
  for (const p of parts.slice(0, -1)) {
    acc = joinPath(acc, p);
    chain.push(acc);
  }
  const out: string[] = [];
  for (let i = chain.length - 1; i >= 0 && out.length < max; i--) {
    // Stop at the source: a pattern scoped above it would reach outside what's backed up.
    if (src && chain[i].length < src.length) break;
    out.push(chain[i]);
  }
  return out;
}

/**
 * The exclude patterns offered for a clicked path, widest-last: exactly this path, a folder's
 * contents only, the name anywhere, the name under a nearer ancestor, a file extension. The
 * first option is the default. `{ancestor}/**\/{name}` follows restic's `**` rule.
 */
export function scopeOptions(path: string, kind: SourceKind, sources: string[], windows = false): ScopeOption[] {
  const name = baseName(path);
  const opts: ScopeOption[] = [{ id: "exact", label: "This exact item", pattern: escapeGlob(path, windows) }];
  if (kind === "dir") {
    opts.push({ id: "contents", label: "Everything inside it, keeping the folder itself", pattern: `${escapeGlob(path, windows)}/*` });
  }
  opts.push({
    id: "name",
    label: kind === "dir" ? `Any folder named “${name}”, anywhere` : `Any file named “${name}”, anywhere`,
    pattern: escapeGlob(name, windows),
  });
  for (const a of ancestorsBelowSource(path, sources, 3)) {
    opts.push({
      id: `under:${a}`,
      label: `“${name}” anywhere under ${baseName(a) || a}`,
      pattern: `${escapeGlob(a, windows)}/**/${escapeGlob(name, windows)}`,
    });
  }
  const dot = name.lastIndexOf(".");
  if (kind !== "dir" && dot > 0 && dot < name.length - 1) {
    opts.push({ id: "ext", label: `All “.${name.slice(dot + 1)}” files`, pattern: `*${escapeGlob(name.slice(dot), windows)}` });
  }
  return opts;
}

/** One line describing what an edit changed, for the confirmation shown in the preview card. */
export function summarizeEdit(o: EditOutcome): string {
  const parts: string[] = [];
  if (o.addedSources.length) parts.push(`added source ${o.addedSources.join(", ")}`);
  if (o.removedSources.length) parts.push(`removed source ${o.removedSources.join(", ")}`);
  if (o.addedRules.length) parts.push(`added ${o.addedRules.length === 1 ? "pattern" : `${o.addedRules.length} patterns`}`);
  if (o.removedRules.length) parts.push(`removed ${o.removedRules.length === 1 ? "rule" : `${o.removedRules.length} rules`}`);
  if (o.broughtBack && o.broughtBack.count > 0) {
    const { count, capped } = o.broughtBack;
    parts.push(`also brought back ${capped ? "at least " : ""}${count} ${count === 1 ? "item" : "items"} other rules exclude`);
  }
  return parts.length ? parts.join("; ") : "no change";
}
