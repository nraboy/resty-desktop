import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  treeClassify,
  treeKinds,
  treeListDir,
  treeScanCancel,
  treeScanStart,
  treeEnv,
  treeSupported,
} from "../lib/invoke";
import {
  ancestorsOf,
  baseName,
  displayState,
  joinPath,
  flattenRows,
  reasonLines,
  toCanonical,
  toNative,
  topLevelSources,
  visibleWindow,
  type DisplayState,
  type NodeInfo,
  type NodeVerdict,
  type RuleOrigin,
  type RuleProblem,
  type ScanResult,
  type SourceKind,
  type TreeEnv,
  type TreeRulesInput,
} from "../lib/planTree";
import { ChevronDownIcon, ChevronRightIcon, CheckIcon, FileIcon, FolderIcon, LinkIcon, WarningIcon } from "./icons";
import { isWindows } from "../lib/utils";
import PlanTreeEditDialog, { type EditTarget } from "./PlanTreeEditDialog";
import PlanTreeExportDialog from "./PlanTreeExportDialog";
import type { EditOutcome } from "../lib/planTreeEdit";
import Spinner from "./Spinner";
import Tooltip from "./Tooltip";

const ROW_H = 26;
const VIEW_H = 340;
/** Paths classified per backend call. */
const CLASSIFY_CHUNK = 1500;
/** How often streamed scan results are folded into the view. */
const FLUSH_MS = 100;

type RootMode = "sources" | "filesystem";


interface ScanEventPayload extends ScanResult {
  scanId: number;
  path: string;
}

interface Props {
  rules: TreeRulesInput;
  /** Set when the plan can't be previewed (e.g. it uses path-list files); shown instead of the tree. */
  unavailableReason?: string;
  /** Called with the plan's new sources and patterns when the user confirms an include/exclude
   * from the tree. When absent the tree is read-only. */
  onApplyEdit?: (outcome: EditOutcome) => void;
  /** The last edit made from the tree, with a way to take it back. */
  lastEdit?: { summary: string; onUndo: () => void } | null;
  /** False while the preview's tab is hidden: nothing is loaded or scanned until it's shown
   * (the component stays mounted so its state survives a tab switch). Defaults to true. */
  active?: boolean;
}

const STATE_LABEL: Record<DisplayState, string> = {
  included: "Included",
  partial: "Partly included",
  excluded: "Excluded",
  contains: "Contains backed-up items",
  unverified: "Included (not fully checked)",
  pending: "Checking",
};

/** The checkbox-style marker: filled = included, dash = partly, empty = excluded. Read-only. */
function StateBox({ state }: { state: DisplayState }) {
  const base = "inline-flex items-center justify-center w-4 h-4 rounded-[3px] border flex-shrink-0";
  switch (state) {
    case "included":
      return (
        <span className={`${base} bg-blue-600 border-blue-600 text-white`} aria-hidden="true">
          <CheckIcon className="w-3 h-3" />
        </span>
      );
    case "unverified":
      return (
        <span className={`${base} bg-blue-600/70 border-blue-500 border-dashed text-white`} aria-hidden="true">
          <CheckIcon className="w-3 h-3" />
        </span>
      );
    case "partial":
    case "contains":
      return (
        <span className={`${base} border-blue-500 bg-blue-900/40`} aria-hidden="true">
          <span className="block w-2 h-[2px] bg-blue-400 rounded" />
        </span>
      );
    case "pending":
      return <span className={`${base} border-gray-600 animate-pulse`} aria-hidden="true" />;
    default:
      return <span className={`${base} border-gray-600`} aria-hidden="true" />;
  }
}

/**
 * Read-only preview of what a backup plan includes: a filesystem tree whose rows are marked
 * included / partly included / excluded, computed by restic's own matching rules (the Rust
 * engine in plan_rules.rs), with a tooltip saying which rule decided each row.
 *
 * Nothing is persisted. Listings and verdicts live in refs for the lifetime of this component;
 * only the visible window of rows is rendered, so a folder with thousands of entries stays
 * cheap, and subtree scans (is anything inside excluded?) run in the backend only for the
 * included folders you can see.
 */
export default function PlanTreePreview({ rules, unavailableReason, onApplyEdit, lastEdit, active = true }: Props) {
  const [supported, setSupported] = useState<boolean | null>(null);
  // Open by default: the preview is the point of the card. It can still be collapsed, and nothing
  // is read from disk until the card is open and the plan has sources.
  const [open, setOpen] = useState(true);
  const [mode, setMode] = useState<RootMode>("sources");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [scrollTop, setScrollTop] = useState(0);
  const [problems, setProblems] = useState<RuleProblem[]>([]);
  const [editTarget, setEditTarget] = useState<EditTarget | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  // The data lives in refs and a counter forces a re-render: scan results stream in at a high
  // rate and each one would otherwise copy these maps.
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const nodes = useRef(new Map<string, NodeInfo>());
  const verdicts = useRef(new Map<string, NodeVerdict>());
  const scans = useRef(new Map<string, ScanResult>());
  const origins = useRef(new Map<number, RuleOrigin>());

  // Rules are debounced: typing a pattern shouldn't rescan on every keystroke.
  const rulesKey = JSON.stringify(rules);
  const [activeKey, setActiveKey] = useState(rulesKey);
  useEffect(() => {
    const t = setTimeout(() => setActiveKey(rulesKey), 350);
    return () => clearTimeout(t);
  }, [rulesKey]);
  const activeRules = useMemo(() => JSON.parse(activeKey) as TreeRulesInput, [activeKey]);
  const rulesRef = useRef(activeRules);
  rulesRef.current = activeRules;

  const generation = useRef(0);
  const inflight = useRef(new Set<string>());
  const latestScan = useRef(0);
  const minScan = useRef(0);
  const scanTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);

  // Where the tree starts (`/`, or a drive list on Windows) comes from the backend; the OS's
  // spelling of paths decides how the plan's sources are read (`C:\\Users\\me` ↔ `C:/Users/me`).
  const windows = isWindows();
  const [env, setEnv] = useState<TreeEnv | null>(null);
  useEffect(() => {
    treeSupported().then(setSupported).catch(() => setSupported(false));
    treeEnv().then(setEnv).catch(() => {});
  }, []);

  const roots = useMemo(
    () =>
      mode === "sources"
        ? topLevelSources(activeRules.sources.map((s) => toCanonical(s, windows))).map((p) => ({
            path: p,
            // Shown the way the OS writes it.
            name: toNative(p, windows),
          }))
        : env && env.roots.length > 0
          ? env.roots.map((r) => ({ path: r.path, name: env.windows ? r.label : `${r.label} (${r.path})` }))
          : [{ path: "/", name: "Computer (/)" }],
    [mode, activeRules.sources, windows, env],
  );

  // ── data loading ──────────────────────────────────────────────────────────

  const classify = useCallback(async (paths: string[], gen: number) => {
    for (let i = 0; i < paths.length; i += CLASSIFY_CHUNK) {
      const chunk = paths.slice(i, i + CLASSIFY_CHUNK);
      let res;
      try {
        res = await treeClassify(rulesRef.current, chunk);
      } catch {
        return;
      }
      if (gen !== generation.current) return;
      for (const v of res.nodes) verdicts.current.set(v.path, v);
      origins.current = new Map(res.origins.map((o) => [o.id, o]));
      setProblems(res.problems);
      bump();
    }
  }, []);

  const listFolder = useCallback(
    async (path: string) => {
      const info = nodes.current.get(path);
      if (!info || info.kind !== "dir" || info.children || info.loading) return;
      info.loading = true;
      bump();
      const gen = generation.current;
      let listing;
      try {
        listing = await treeListDir(path);
      } catch (e) {
        info.loading = false;
        info.error = String(e);
        info.children = [];
        bump();
        return;
      }
      info.loading = false;
      if (listing.error) {
        info.error = listing.error;
        info.children = [];
        bump();
        return;
      }
      const children: string[] = [];
      for (const e of listing.entries) {
        const child = joinPath(path, e.name);
        children.push(child);
        if (!nodes.current.has(child)) nodes.current.set(child, { path: child, name: e.name, kind: e.kind });
      }
      info.children = children;
      info.total = listing.total;
      info.truncated = listing.truncated;
      bump();
      await classify(children, gen);
    },
    [classify],
  );

  /** Re-derives everything that depends on the rules or the roots. Listings are kept. */
  const refresh = useCallback(async () => {
    const gen = ++generation.current;
    minScan.current = latestScan.current + 1;
    inflight.current.clear();
    verdicts.current.clear();
    scans.current.clear();
    treeScanCancel().catch(() => {});
    bump();

    // Make sure every root has a node (sources: is it a folder, a file, or gone?).
    const missingRoots = roots.filter((r) => !nodes.current.has(r.path));
    if (missingRoots.length > 0) {
      if (mode === "sources") {
        const kinds = await treeKinds(missingRoots.map((r) => r.path), true).catch(() => []);
        missingRoots.forEach((r, i) => {
          // The node keeps its real folder name (it may also appear nested in the whole-filesystem
          // view); a source root's full path is only its label as a root.
          nodes.current.set(r.path, { path: r.path, name: baseName(r.path), kind: (kinds[i] ?? "missing") as SourceKind });
        });
      } else {
        for (const r of missingRoots) nodes.current.set(r.path, { path: r.path, name: baseName(r.path), kind: "dir" });
      }
      if (gen !== generation.current) return;
    }

    // Open the roots (and, browsing the whole filesystem, the way down to each source).
    const toOpen = new Set<string>(roots.map((r) => r.path));
    if (mode === "filesystem") {
      for (const s of topLevelSources(rulesRef.current.sources.map((x) => toCanonical(x, windows))).slice(0, 10)) {
        for (const a of ancestorsOf(s)) toOpen.add(a);
        toOpen.add(s);
      }
    }
    setExpanded((prev) => {
      const next = new Set(prev);
      toOpen.forEach((p) => next.add(p));
      return next;
    });

    await classify(Array.from(nodes.current.keys()), gen);
    if (gen !== generation.current) return;
    // Anything expanded but not yet listed (first open, or new roots).
    for (const p of toOpen) await listFolder(p);
  }, [classify, listFolder, mode, roots, windows]);

  // Start (and restart on any change of rules or roots) once the preview is open.
  useEffect(() => {
    if (!open || !supported || !active) return;
    refresh();
  }, [open, supported, active, activeKey, mode, refresh]);

  // Cancel any running scan when the component goes away.
  useEffect(
    () => () => {
      generation.current++;
      treeScanCancel().catch(() => {});
    },
    [],
  );

  // ── streamed scan results ─────────────────────────────────────────────────

  useEffect(() => {
    if (!open || !supported || !active) return;
    let buffer: ScanEventPayload[] = [];
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<ScanEventPayload>("tree:scan", (ev) => {
      buffer.push(ev.payload);
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    const timer = setInterval(() => {
      if (buffer.length === 0) return;
      const batch = buffer;
      buffer = [];
      let changed = false;
      for (const ev of batch) {
        if (ev.scanId < minScan.current) continue; // from before the last rules change
        latestScan.current = Math.max(latestScan.current, ev.scanId);
        scans.current.set(ev.path, ev);
        inflight.current.delete(ev.path);
        changed = true;
      }
      if (changed) bump();
    }, FLUSH_MS);
    return () => {
      cancelled = true;
      unlisten?.();
      clearInterval(timer);
    };
  }, [open, supported, active]);

  // ── what to render ────────────────────────────────────────────────────────

  const rows = flattenRows(roots, nodes.current, expanded);
  const { start, end } = visibleWindow(rows.length, scrollTop, VIEW_H, ROW_H);

  // Scan the included folders in view (plus overscan) that haven't been checked yet. A new scan
  // supersedes the running one in the backend, so everything still unresolved is re-sent.
  const scanKey = `${start}:${end}:${rows.length}:${scans.current.size}:${verdicts.current.size}`;
  useEffect(() => {
    if (!open || !supported || !active) return;
    if (scanTimer.current) clearTimeout(scanTimer.current);
    scanTimer.current = setTimeout(async () => {
      const gen = generation.current;
      const wanted: string[] = [];
      for (let i = start; i < end; i++) {
        const row = rows[i];
        if (row?.type !== "node" || row.kind !== "dir") continue;
        const v = verdicts.current.get(row.path);
        if (v?.included && !v.containsSources && !scans.current.has(row.path)) wanted.push(row.path);
      }
      const fresh = wanted.filter((p) => !inflight.current.has(p));
      if (fresh.length === 0) return;
      const dirs = [...wanted, ...Array.from(inflight.current).filter((p) => !wanted.includes(p))];
      dirs.forEach((p) => inflight.current.add(p));
      try {
        const id = await treeScanStart(rulesRef.current, dirs);
        if (gen === generation.current) latestScan.current = Math.max(latestScan.current, id);
      } catch {
        dirs.forEach((p) => inflight.current.delete(p));
      }
    }, 150);
    return () => {
      if (scanTimer.current) clearTimeout(scanTimer.current);
    };
    // `rows` is derived from the refs, so `scanKey` stands in for it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, supported, active, scanKey]);

  const toggle = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
    if (!expanded.has(path)) listFolder(path);
  };

  const openEdit = (path: string, kind: SourceKind) => {
    const isSource = rules.sources.some((s) => toCanonical(s, windows) === path);
    setEditTarget({
      path,
      name: baseName(path),
      kind,
      verdict: verdicts.current.get(path),
      isSource,
    });
  };

  const reload = () => {
    nodes.current.clear();
    refresh();
  };

  if (supported === false) return null;

  const emptySources = mode === "sources" && roots.length === 0;
  const visible = rows.slice(start, end);

  return (
    <div className="bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center justify-between text-left"
        aria-expanded={open}
      >
        <span>
          <span className="block text-sm font-medium text-gray-300">Preview</span>
          <span className="block text-xs text-gray-500 mt-0.5">
            See exactly which folders and files this plan backs up, and why anything is left out.
          </span>
        </span>
        <span className="text-gray-500 flex-shrink-0 ml-3">
          {open ? <ChevronDownIcon className="w-4 h-4" /> : <ChevronRightIcon className="w-4 h-4" />}
        </span>
      </button>

      {open && (
        <div className="mt-4">
          {unavailableReason ? (
            <p className="text-sm text-gray-500">{unavailableReason}</p>
          ) : (
            <>
              <div className="flex items-center justify-between gap-3 mb-3 flex-wrap">
                <div className="flex rounded-lg overflow-hidden border border-gray-700">
                  {(["sources", "filesystem"] as const).map((m) => (
                    <button
                      key={m}
                      type="button"
                      onClick={() => setMode(m)}
                      className={`px-3 py-1 text-xs font-medium transition-colors ${
                        mode === m ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"
                      }`}
                    >
                      {m === "sources" ? "Sources" : "Whole filesystem"}
                    </button>
                  ))}
                </div>
                <div className="flex items-center gap-4 text-xs text-gray-500">
                  <span className="inline-flex items-center gap-1.5"><StateBox state="included" /> Included</span>
                  <span className="inline-flex items-center gap-1.5"><StateBox state="partial" /> Partly</span>
                  <span className="inline-flex items-center gap-1.5"><StateBox state="excluded" /> Excluded</span>
                  <button type="button" onClick={reload} className="text-gray-400 hover:text-gray-50 underline-offset-2 hover:underline">
                    Refresh
                  </button>
                  <button type="button" onClick={() => setExportOpen(true)} className="text-gray-400 hover:text-gray-50 underline-offset-2 hover:underline">
                    Use with restic CLI…
                  </button>
                </div>
              </div>

              {lastEdit && (
                <p className="mb-3 text-xs text-gray-400" role="status">
                  Updated your plan: {lastEdit.summary}.{" "}
                  <button type="button" onClick={lastEdit.onUndo} className="text-blue-400 hover:text-blue-300 underline-offset-2 hover:underline">
                    Undo
                  </button>
                </p>
              )}

              {problems.length > 0 && (
                <ul className="mb-3 space-y-1">
                  {problems.map((p, i) => (
                    <li key={i} className="flex items-start gap-1.5 text-xs text-amber-400">
                      <WarningIcon className="w-3.5 h-3.5 flex-shrink-0 mt-px" />
                      <span><span className="font-mono">{p.what}</span>: {p.error}</span>
                    </li>
                  ))}
                </ul>
              )}

              {emptySources ? (
                <p className="text-sm text-gray-500 text-center py-6">
                  Add a source folder above to preview what will be backed up, or browse the whole filesystem.
                </p>
              ) : (
                <div
                  ref={viewportRef}
                  role="tree"
                  aria-label="Files and folders in this backup plan"
                  className="relative overflow-auto rounded-lg border border-gray-800 bg-gray-950/40"
                  style={{ height: VIEW_H }}
                  onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
                >
                  <div style={{ height: rows.length * ROW_H, position: "relative", minWidth: "max-content" }}>
                    {visible.map((row, i) => {
                      const top = (start + i) * ROW_H;
                      const indent = 8 + row.depth * 16;
                      if (row.type === "note") {
                        return (
                          <div
                            key={row.key}
                            className={`absolute left-0 right-0 flex items-center text-xs ${
                              row.tone === "error" ? "text-amber-400" : "text-gray-500"
                            }`}
                            style={{ top, height: ROW_H, paddingLeft: indent + 22 }}
                          >
                            {row.text}
                          </div>
                        );
                      }
                      const verdict = verdicts.current.get(row.path);
                      const scan = scans.current.get(row.path);
                      const missing = row.kind === "missing";
                      const state: DisplayState = missing ? "excluded" : displayState(row.kind, verdict, scan);
                      const lines = reasonLines(state, row.path, verdict, scan, origins.current, row.kind);
                      const dim = state === "excluded" || missing;
                      const isDir = row.kind === "dir";
                      return (
                        <div
                          key={row.path}
                          role="treeitem"
                          aria-level={row.depth + 1}
                          aria-expanded={isDir ? row.expanded : undefined}
                          aria-label={`${row.name}: ${missing ? "missing" : STATE_LABEL[state]}`}
                          className="absolute left-0 right-0 flex items-center gap-1.5 hover:bg-gray-800/40"
                          style={{ top, height: ROW_H, paddingLeft: indent }}
                        >
                          {isDir ? (
                            <button
                              type="button"
                              onClick={() => toggle(row.path)}
                              className="w-4 h-4 flex items-center justify-center text-gray-500 hover:text-gray-300 flex-shrink-0"
                              aria-label={row.expanded ? `Collapse ${row.name}` : `Expand ${row.name}`}
                            >
                              {row.loading ? (
                                <Spinner className="w-3 h-3 text-blue-400" />
                              ) : row.expanded ? (
                                <ChevronDownIcon className="w-3.5 h-3.5" />
                              ) : (
                                <ChevronRightIcon className="w-3.5 h-3.5" />
                              )}
                            </button>
                          ) : (
                            <span className="w-4 flex-shrink-0" />
                          )}
                          {onApplyEdit && state !== "pending" ? (
                            <button
                              type="button"
                              onClick={() => openEdit(row.path, row.kind)}
                              className="rounded-[3px] hover:ring-2 hover:ring-blue-500/40 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 flex-shrink-0"
                              title={
                                verdict?.included
                                  ? rules.sources.some((src) => src.trim() === row.path)
                                    ? "Remove this source…"
                                    : "Exclude…"
                                  : "Include…"
                              }
                              aria-label={`${row.name}: ${verdict?.included ? "exclude…" : "include…"}`}
                            >
                              <StateBox state={state} />
                            </button>
                          ) : (
                            <StateBox state={state} />
                          )}
                          <span className={`flex-shrink-0 ${dim ? "text-gray-500" : "text-gray-400"}`}>
                            {row.kind === "symlink" ? (
                              <LinkIcon className="w-4 h-4" />
                            ) : isDir ? (
                              <FolderIcon className="w-4 h-4" />
                            ) : (
                              <FileIcon className="w-4 h-4" />
                            )}
                          </span>
                          <Tooltip content={<div className="space-y-1">{lines.map((l, j) => <p key={j}>{l}</p>)}</div>}>
                            <span
                              className={`text-sm truncate cursor-help ${dim ? "text-gray-500" : "text-gray-200"} ${
                                row.kind === "symlink" ? "italic" : ""
                              }`}
                              style={{ maxWidth: 520 }}
                            >
                              {row.name}
                            </span>
                          </Tooltip>
                          {missing && <span className="text-xs text-amber-400 ml-1">doesn&apos;t exist</span>}
                          {state === "unverified" && <span className="text-xs text-gray-500 ml-1">not fully checked</span>}
                          {state === "partial" && <span className="text-xs text-gray-500 ml-1">some items excluded</span>}
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
              <p className="text-xs text-gray-500 mt-2">
                Computed with the same rules restic applies. Hover a name to see why{onApplyEdit ? "; click a box to include or exclude it" : ""}.
              </p>
            </>
          )}
        </div>
      )}

      {onApplyEdit && (
        <PlanTreeEditDialog target={editTarget} rules={rules} onClose={() => setEditTarget(null)} onApply={onApplyEdit} />
      )}
      <PlanTreeExportDialog open={exportOpen} rules={rules} onClose={() => setExportOpen(false)} />
    </div>
  );
}
