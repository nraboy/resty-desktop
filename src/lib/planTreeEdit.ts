// Pure helpers for editing a plan from the tree (components/PlanTreeEditDialog.tsx): the
// pattern choices offered for a clicked path. The edits themselves are synthesised and verified
// by the Rust engine (commands/plan_edit.rs); the patterns offered here are plain restic
// patterns and are only ever applied through that verification.

import type { SourceKind } from "./planTree";

/** The result of `tree_plan_edit`: the plan's sources and patterns after the edit. */
export interface EditOutcome {
  sources: string[];
  excludes: string[];
  addedRules: string[];
  addedSources: string[];
  removedSources: string[];
}

export interface EditAction {
  kind: "exclude" | "include";
  path: string;
  pattern?: string;
  /** For `include` below an excluded folder: just the item (default) or the whole folder. */
  scope?: "item" | "folder";
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
export function escapeGlob(s: string): string {
  return s.replace(/[\\[\]*?]/g, (c) => `\\${c}`);
}

export interface ScopeOption {
  id: string;
  label: string;
  /** The restic pattern this choice adds. */
  pattern: string;
}

const baseName = (p: string) => p.split("/").filter(Boolean).pop() ?? p;

/** The folders between a source and the path's own folder, nearest first (at most `max`). */
function ancestorsBelowSource(path: string, sources: string[], max: number): string[] {
  const src = sources
    .filter((s) => path === s || path.startsWith(s + "/"))
    .sort((a, b) => b.length - a.length)[0];
  const parts = path.split("/").filter(Boolean);
  const out: string[] = [];
  let acc = "";
  const chain: string[] = [];
  for (const p of parts.slice(0, -1)) {
    acc += "/" + p;
    chain.push(acc);
  }
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
export function scopeOptions(path: string, kind: SourceKind, sources: string[]): ScopeOption[] {
  const name = baseName(path);
  const opts: ScopeOption[] = [{ id: "exact", label: "This exact item", pattern: escapeGlob(path) }];
  if (kind === "dir") {
    opts.push({ id: "contents", label: "Everything inside it, keeping the folder itself", pattern: `${escapeGlob(path)}/*` });
  }
  opts.push({
    id: "name",
    label: kind === "dir" ? `Any folder named “${name}”, anywhere` : `Any file named “${name}”, anywhere`,
    pattern: escapeGlob(name),
  });
  for (const a of ancestorsBelowSource(path, sources, 3)) {
    opts.push({
      id: `under:${a}`,
      label: `“${name}” anywhere under ${baseName(a) || a}`,
      pattern: `${escapeGlob(a)}/**/${escapeGlob(name)}`,
    });
  }
  const dot = name.lastIndexOf(".");
  if (kind !== "dir" && dot > 0 && dot < name.length - 1) {
    opts.push({ id: "ext", label: `All “.${name.slice(dot + 1)}” files`, pattern: `*${escapeGlob(name.slice(dot))}` });
  }
  return opts;
}

/** One line describing what an edit changed, for the confirmation shown in the preview card. */
export function summarizeEdit(o: EditOutcome): string {
  const parts: string[] = [];
  if (o.addedSources.length) parts.push(`added source ${o.addedSources.join(", ")}`);
  if (o.removedSources.length) parts.push(`removed source ${o.removedSources.join(", ")}`);
  if (o.addedRules.length) parts.push(`added ${o.addedRules.length === 1 ? "pattern" : `${o.addedRules.length} patterns`}`);
  return parts.length ? parts.join("; ") : "no change";
}
