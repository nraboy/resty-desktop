// Pure logic behind the plan editor's file-tree preview (components/PlanTreePreview.tsx):
// what state a row shows, the plain-language reason behind it, and the flat list of rows a
// virtualised tree renders. The decisions themselves come from the Rust rule engine
// (commands/plan_rules.rs, a port of restic's matching); nothing here re-implements them.

import type { ExcludeFile } from "./types";

export type EntryKind = "dir" | "file" | "symlink" | "other";

export interface TreeEntry {
  name: string;
  kind: EntryKind;
  /** Hidden by the platform's convention; still backed up like any other item. */
  hidden?: boolean;
}

export interface TreeListing {
  entries: TreeEntry[];
  total: number;
  truncated: boolean;
  error?: string | null;
}

/** The plan's include-deciding parts, as sent to the backend (mirrors `TreeRulesInput`). */
export interface TreeRulesInput {
  sources: string[];
  /** Sources contributed by path-list files, already expanded (`tree_list_files`), in the tree's
   * spelling. Read-only: edits never add, remove or rewrite them. */
  listSources?: string[];
  /** The path-list files themselves; only the CLI export reads them. */
  filesFrom?: string[];
  excludes: string[];
  excludeIfPresent: string[];
  excludeCaches: boolean;
  excludeFiles: ExcludeFile[];
  /** The user's restic is older than 0.19 and applies the excludes to the paths you pick directly
   * too (`TreeEnv.resticJudgesSources`); the backend judges them the same way. */
  judgeSources?: boolean;
}

export interface RuleOrigin {
  id: number;
  pattern: string;
  file?: string | null;
  line?: number | null;
  /** From an `--iexclude-file` (case-insensitive list). */
  ignoreCase?: boolean;
}

/** One source a path-list file contributes: `path` came from `line` of `file` (`pattern` is the
 * line itself, a glob when it differs from `path`). */
export interface ListEntry {
  path: string;
  file: string;
  line: number;
  pattern: string;
}

export interface ListFilesResult {
  entries: ListEntry[];
  problems: RuleProblem[];
  /** Stopped at the entry cap. */
  truncated: boolean;
}

export interface RuleProblem {
  what: string;
  error: string;
}

export interface NodeVerdict {
  path: string;
  included: boolean;
  reason?: { kind: "pattern" | "marker" | "outsideSources"; ruleId?: number | null; spec?: string | null } | null;
  /** The rejected ancestor, when the reason applies to a folder above this path. */
  at?: string | null;
  reincludedBy?: number | null;
  /** Not backed up itself, but a source lies below it. */
  containsSources: boolean;
}

/** What the backend knows about this computer: where the tree starts and the home folder. */
export interface TreeRoot {
  label: string;
  path: string;
}

export interface TreeEnv {
  windows: boolean;
  roots: TreeRoot[];
  home?: string | null;
  /** restic < 0.19 (restic#5767): an exclude pattern or marker can also drop a folder or file you
   * picked directly, not only what is inside it. Absent/false = the 0.19 behaviour. */
  resticJudgesSources?: boolean;
}

export interface Classification {
  nodes: NodeVerdict[];
  origins: RuleOrigin[];
  problems: RuleProblem[];
}

/** Result of scanning an included folder's subtree for excluded items. */
export interface ScanResult {
  partial: boolean;
  /** Definitive: an exclusion was found, or everything was looked at. */
  complete: boolean;
  firstExcluded?: string | null;
  /** Some folder couldn't be read, so "nothing excluded" can't be asserted. */
  unreadable: boolean;
  scanned: number;
}

export type SourceKind = EntryKind | "missing";

/** A folder or file in the tree, as listed from disk. */
export interface NodeInfo {
  path: string;
  name: string;
  kind: SourceKind;
  /** Child paths once the folder has been listed. */
  children?: string[];
  loading?: boolean;
  total?: number;
  truncated?: boolean;
  /** Why the folder couldn't be listed. */
  error?: string | null;
  /** Hidden by the platform's convention (dot-name, or the Windows hidden attribute). */
  hidden?: boolean;
}

export type DisplayState =
  /** Backed up, and (for a folder) nothing inside is excluded. */
  | "included"
  /** Backed up, but something inside is excluded. */
  | "partial"
  /** Not backed up. */
  | "excluded"
  /** Not backed up itself, but a source lies below it. */
  | "contains"
  /** Backed up; a large folder that wasn't fully checked within the time budget. */
  | "unverified"
  /** Waiting on a verdict or a scan. */
  | "pending";

export function displayState(
  kind: SourceKind,
  verdict: NodeVerdict | undefined,
  scan: ScanResult | undefined,
): DisplayState {
  if (!verdict) return "pending";
  if (verdict.containsSources) return "contains";
  if (!verdict.included) return "excluded";
  if (kind !== "dir") return "included";
  if (!scan) return "pending";
  if (scan.partial) return "partial";
  return scan.complete ? "included" : "unverified";
}

// ── paths ───────────────────────────────────────────────────────────────────
//
// The tree works on one spelling everywhere: `/` separators, with a drive root written `C:/` on
// Windows. The backend accepts and returns that form (and the OS's own spelling for the plan's
// sources — `toCanonical` bridges the two).

/** The root a path starts at — `/` or a drive such as `C:/` — or null for a relative path. */
export function rootOf(path: string): string | null {
  const drive = /^[A-Za-z]:\//.exec(path);
  if (drive) return drive[0];
  return path.startsWith("/") ? "/" : null;
}

export const isRootPath = (path: string): boolean => rootOf(path) === path;

/** `name` inside `dir` (a root already ends in a separator). */
export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

/** Is `path` strictly inside `dir`? */
export function isInside(path: string, dir: string): boolean {
  if (path === dir) return false;
  return dir.endsWith("/") ? path.startsWith(dir) : path.startsWith(dir + "/");
}

/** The tree's spelling of a path as the OS writes it: on Windows `\` becomes `/`; a trailing
 * separator is dropped except on a root. */
export function toCanonical(path: string, windows: boolean): string {
  let p = path.trim();
  if (windows) p = p.replace(/\\/g, "/");
  if (p.length > 1 && p.endsWith("/") && !isRootPath(p)) p = p.replace(/\/+$/, "");
  return p;
}

/** A canonical path as the OS spells it (`C:\Users\me` on Windows). */
export function toNative(path: string, windows: boolean): string {
  return windows ? path.replace(/\//g, "\\") : path;
}

/** The last component, or the path itself for a root. */
export function baseName(path: string): string {
  const parts = path.split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

/** The path below `base` (or the whole path when it isn't below it). */
export function relativeTo(path: string, base: string): string {
  if (isInside(path, base)) return path.slice(base.endsWith("/") ? base.length : base.length + 1);
  return path;
}

function describeRule(origin: RuleOrigin | undefined): string {
  if (!origin) return "a rule";
  if (origin.file) {
    return `\`${origin.pattern}\` (line ${origin.line ?? "?"} of ${origin.file})`;
  }
  return `\`${origin.pattern}\``;
}

/** Plain-language lines explaining a row's state — the tooltip text. */
export function reasonLines(
  state: DisplayState,
  path: string,
  verdict: NodeVerdict | undefined,
  scan: ScanResult | undefined,
  origins: Map<number, RuleOrigin>,
  kind: SourceKind = "dir",
): string[] {
  if (kind === "missing") return ["This path doesn't exist."];
  if (!verdict) return ["Checking…"];
  const lines: string[] = [];
  switch (state) {
    case "contains":
      lines.push("Not backed up itself, but contains a folder or file that is.");
      break;
    case "excluded": {
      const r = verdict.reason;
      const where = verdict.at && verdict.at !== path ? ` because its folder ${verdict.at} is excluded` : "";
      if (r?.kind === "pattern") {
        lines.push(`Excluded${where || ""} by ${describeRule(origins.get(r.ruleId ?? -1))}.`);
      } else if (r?.kind === "marker") {
        lines.push(
          `Excluded${where || ""}: its folder contains a \`${(r.spec ?? "").split(":")[0]}\` file, which excludes everything beside it.`,
        );
      } else {
        lines.push("Not inside any backup source.");
      }
      break;
    }
    case "partial":
      lines.push("Backed up, but some items inside are excluded.");
      if (scan?.firstExcluded) lines.push(`For example: ${relativeTo(scan.firstExcluded, path)}`);
      break;
    case "unverified":
      lines.push(
        `Backed up. ${scan?.scanned.toLocaleString() ?? "Many"} items were checked without finding an exclusion, but this folder is too large to check completely here.`,
      );
      break;
    case "pending":
      lines.push(verdict.included ? "Backed up. Checking what's inside…" : "Checking…");
      break;
    default:
      lines.push(kind === "dir" ? "Backed up, with everything inside." : "Backed up.");
  }
  if (verdict.included && verdict.reincludedBy != null) {
    lines.push(`Re-included by ${describeRule(origins.get(verdict.reincludedBy))}.`);
  }
  if (scan?.unreadable) {
    lines.push("Some folders inside can't be read (permissions). On macOS this may need Full Disk Access.");
  }
  return lines;
}

/** One line of the flattened tree. */
export type Row =
  | { type: "node"; path: string; name: string; kind: SourceKind; depth: number; expanded: boolean; loading: boolean }
  | {
      type: "note";
      key: string;
      text: string;
      depth: number;
      tone: "info" | "error";
      /** The note offers to show the hidden items it counts. */
      action?: "showHidden";
    };

/** How hidden items are treated when flattening the tree. */
export interface HiddenOptions {
  showHidden: boolean;
  /** Hidden items that are shown regardless: sources and the folders leading to them. */
  pinned: ReadonlySet<string>;
  /** Is this path backed up? Unknown (undefined) until its verdict arrives. */
  included?: (path: string) => boolean | undefined;
}

/** "3 hidden items not shown — 2 are backed up". */
export function hiddenNoteText(hidden: number, backedUp: number): string {
  const items = `${hidden.toLocaleString()} hidden ${hidden === 1 ? "item" : "items"} not shown`;
  if (backedUp === 0) return `${items} — none are backed up`;
  if (backedUp === hidden) return `${items} — ${hidden === 1 ? "it is" : "all are"} backed up`;
  return `${items} — ${backedUp.toLocaleString()} ${backedUp === 1 ? "is" : "are"} backed up`;
}

/**
 * Flattens the visible part of the tree — roots, plus the children of every expanded folder
 * that has been listed — into rows. `roots` are shown in order; a folder's `children` come from
 * its listing and are already sorted. With `showHidden` off, hidden children are left out (unless
 * pinned) and the folder ends with a note saying how many, and how many of those are backed up.
 */
export function flattenRows(
  roots: { path: string; name?: string }[],
  nodes: Map<string, NodeInfo>,
  expanded: Set<string>,
  hiddenOpts?: HiddenOptions,
): Row[] {
  const rows: Row[] = [];
  const hideHidden = !!hiddenOpts && !hiddenOpts.showHidden;
  const visit = (path: string, depth: number, nameOverride?: string) => {
    const info = nodes.get(path);
    if (!info) return;
    const isOpen = info.kind === "dir" && expanded.has(path);
    rows.push({
      type: "node",
      path,
      name: nameOverride ?? info.name,
      kind: info.kind,
      depth,
      expanded: isOpen,
      loading: !!info.loading,
    });
    if (!isOpen) return;
    if (info.error) {
      rows.push({ type: "note", key: `${path}\0error`, text: `Can't read this folder: ${info.error}`, depth: depth + 1, tone: "error" });
      return;
    }
    let hiddenCount = 0;
    let hiddenBackedUp = 0;
    for (const child of info.children ?? []) {
      if (hideHidden && nodes.get(child)?.hidden && !hiddenOpts!.pinned.has(child)) {
        hiddenCount++;
        if (hiddenOpts!.included?.(child)) hiddenBackedUp++;
        continue;
      }
      visit(child, depth + 1);
    }
    if (hiddenCount > 0) {
      rows.push({
        type: "note",
        key: `${path}\0hidden`,
        text: hiddenNoteText(hiddenCount, hiddenBackedUp),
        depth: depth + 1,
        tone: "info",
        action: "showHidden",
      });
    }
    if (info.truncated && info.total != null) {
      const shown = info.children?.length ?? 0;
      rows.push({
        type: "note",
        key: `${path}\0more`,
        text: `…and ${(info.total - shown).toLocaleString()} more not shown`,
        depth: depth + 1,
        tone: "info",
      });
    } else if (info.children && info.children.length === 0 && !info.loading) {
      rows.push({ type: "note", key: `${path}\0empty`, text: "Empty folder", depth: depth + 1, tone: "info" });
    }
  };
  for (const r of roots) visit(r.path, 0, r.name);
  return rows;
}

/**
 * Across every folder listed so far: how many hidden items there are and how many of those are
 * backed up — what the "show hidden files" toggle's "!" is based on. Only items whose verdict is
 * known count as backed up.
 */
export function hiddenSummary(
  nodes: Map<string, NodeInfo>,
  included: (path: string) => boolean | undefined,
): { hidden: number; backedUp: number } {
  let hidden = 0;
  let backedUp = 0;
  for (const info of nodes.values()) {
    if (!info.hidden) continue;
    hidden++;
    if (included(info.path)) backedUp++;
  }
  return { hidden, backedUp };
}

/** Folders that must stay visible even when hidden files are off: the sources and every folder
 * on the way down to one. */
export function pinnedPaths(sources: string[]): Set<string> {
  const out = new Set<string>();
  for (const s of sources) {
    out.add(s);
    for (const a of ancestorsOf(s)) out.add(a);
  }
  return out;
}

/** What a left-click on a row's box does. */
export type LeftClick = "removeSource" | "addSource" | "exclude" | "include";

/** The single action a click on a row performs: a source toggles off; an item outside every source
 * becomes one; anything else inside a source flips — an included item is excluded, an excluded
 * one included. Which rules that takes is the backend's call (`tree_plan_edit`). */
export function leftClick(isSource: boolean, verdict: NodeVerdict | undefined): LeftClick {
  if (isSource) return "removeSource";
  if (!verdict || verdict.included) return "exclude";
  return verdict.reason?.kind === "outsideSources" ? "addSource" : "include";
}

/** The wording for a left-click's action: the box's tooltip and the menu's first entry. */
export function leftClickLabel(click: LeftClick, kind: SourceKind): string {
  const what = kind === "dir" ? "folder" : "file";
  switch (click) {
    case "removeSource":
      return `Stop backing up this ${what}`;
    case "addSource":
      return `Back up this ${what}`;
    case "include":
      return "Include";
    case "exclude":
      return "Exclude";
  }
}

/** Everything the right-click menu can do: the left-click's action, plus the two that need a
 * deliberate choice (a pattern to write, or everything inside a folder). */
export type MenuAction = LeftClick | "excludeByPattern" | "includeEverything";

export interface MenuEntry {
  action: MenuAction;
  label: string;
}

/**
 * The right-click menu for a row. The first entry is always the left-click's action, so a
 * right-click is never dead and never at odds with the click. `exactRule` says the plan's own
 * pattern list holds a line that is exactly this path (`hasExactRule`).
 */
export function menuEntries(a: {
  isSource: boolean;
  verdict: NodeVerdict | undefined;
  state: DisplayState;
  kind: SourceKind;
  exactRule: boolean;
}): MenuEntry[] {
  const first = leftClick(a.isSource, a.verdict);
  const head: MenuEntry = { action: first, label: leftClickLabel(first, a.kind) };
  switch (first) {
    case "removeSource":
    case "addSource":
      return [head];
    case "include": {
      const out: MenuEntry[] = [head];
      // The plain click would only delete the exact line; this keeps it and re-includes with `!P`.
      if (a.kind === "dir" && a.exactRule) {
        out.push({ action: "includeEverything", label: "Include, plus everything inside it that other rules exclude" });
      }
      return out;
    }
    case "exclude": {
      const out: MenuEntry[] = [head, { action: "excludeByPattern", label: "Exclude by pattern…" }];
      if (a.state === "partial") out.push({ action: "includeEverything", label: "Include everything inside it" });
      return out;
    }
  }
}

/** Every ancestor folder of `path`, from its root down (excluding `path` itself). */
export function ancestorsOf(path: string): string[] {
  const root = rootOf(path);
  if (!root) return [];
  const parts = path.slice(root.length).split("/").filter(Boolean);
  const out = [root];
  let cur = root;
  for (const p of parts.slice(0, -1)) {
    cur = joinPath(cur, p);
    out.push(cur);
  }
  return out;
}

/** Sources that aren't inside another source: the roots a "Sources" view needs. */
export function topLevelSources(sources: string[]): string[] {
  const clean = Array.from(new Set(sources.map((s) => s.trim()).filter(Boolean)));
  return clean.filter((s) => !clean.some((o) => isInside(s, o)));
}

/** The window of rows to render for a scroll position (fixed row height, with overscan). */
export function visibleWindow(
  rowCount: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan = 6,
): { start: number; end: number } {
  const first = Math.floor(scrollTop / rowHeight);
  const count = Math.ceil(viewportHeight / rowHeight);
  const start = Math.max(0, first - overscan);
  const end = Math.min(rowCount, first + count + overscan);
  return { start, end };
}
