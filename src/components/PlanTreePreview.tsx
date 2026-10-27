import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  treeClassify,
  treeEnv,
  treeKinds,
  treeListDir,
  treeListFiles,
  treePlanEdit,
  treeScanCancel,
  treeScanStart,
} from "../lib/invoke";
import {
  ancestorsOf,
  baseName,
  boxClick,
  displayState,
  flattenRows,
  hiddenSummary,
  joinPath,
  pinnedPaths,
  reasonLines,
  toCanonical,
  toNative,
  topLevelSources,
  visibleWindow,
  type BoxClick,
  type DisplayState,
  type ListEntry,
  type ListFilesResult,
  type NodeInfo,
  type NodeVerdict,
  type RuleOrigin,
  type RuleProblem,
  type ScanResult,
  type SourceKind,
  type TreeEnv,
  type TreeRulesInput,
} from "../lib/planTree";
import { escapeGlob, type EditOutcome } from "../lib/planTreeEdit";
import { isWindows } from "../lib/utils";
import { CARD } from "./plan/types";
import { ChevronDownIcon, ChevronRightIcon, FileIcon, FolderIcon, LinkIcon, WarningIcon } from "./icons";
import PlanTreeEditDialog, { type EditTarget } from "./PlanTreeEditDialog";
import PlanTreeExportDialog from "./PlanTreeExportDialog";
import { SourcePill, STATE_LABEL, StateBox } from "./PlanTreeParts";
import Spinner from "./Spinner";
import Tooltip from "./Tooltip";

const ROW_H = 26;
const DEFAULT_VIEW_H = 440;
const MIN_VIEW_H = 180;
const MAX_VIEW_H = 1100;
const HEIGHT_KEY = "resty.planTree.height";
/** Paths classified per backend call. */
const CLASSIFY_CHUNK = 1500;
/** How often streamed scan results are folded into the view. */
const FLUSH_MS = 100;

type RootMode = "sources" | "filesystem";

const NO_LIST: ListFilesResult = { entries: [], problems: [], truncated: false };
/** Path-list problems listed in full before the rest are summarised. */
const MAX_LIST_PROBLEMS = 5;

interface ScanEventPayload extends ScanResult {
  scanId: number;
  path: string;
}

interface Props {
  rules: TreeRulesInput;
  /** Called with the plan's new sources and patterns when the user confirms (or one-clicks) an
   * include/exclude from the tree. When absent the tree is read-only. */
  onApplyEdit?: (outcome: EditOutcome) => void;
  /** Edits made from the tree, oldest first, each with a summary; `onUndo` takes back the latest. */
  history?: { summary: string }[];
  onUndo?: () => void;
  /** False while the preview's tab is hidden: nothing is loaded or scanned until it's shown
   * (the component stays mounted so its state survives a tab switch). Defaults to true. */
  active?: boolean;
}

const clampHeight = (h: number) => Math.min(MAX_VIEW_H, Math.max(MIN_VIEW_H, Math.round(h)));

function storedHeight(): number {
  try {
    const n = Number(localStorage.getItem(HEIGHT_KEY));
    return n ? clampHeight(n) : DEFAULT_VIEW_H;
  } catch {
    return DEFAULT_VIEW_H;
  }
}

function boxTitle(click: BoxClick, included: boolean | undefined, kind: SourceKind): string {
  const what = kind === "dir" ? "folder" : "file";
  if (click === "removeSource") return `Stop backing up this ${what}`;
  if (click === "addSource") return `Back up this ${what}`;
  return included ? "Exclude…" : "Include…";
}

/**
 * The plan's file tree: what to back up, and a preview of exactly what that includes. It starts
 * at the whole filesystem (`/`, or the drives on Windows) and the plan's sources are the folders
 * and files you pick in it. Each row is marked included / partly included / excluded by restic's
 * own matching rules (the Rust engine in plan_rules.rs), with a tooltip saying which rule decided.
 *
 * Clicking a box edits the plan: a folder outside every source becomes one in a single click (and
 * a source goes away the same way); inside a source a click opens a dialog to pick the pattern.
 * Every edit is computed and verified by the backend and can be undone.
 *
 * Nothing is persisted. Listings and verdicts live in refs for the lifetime of this component;
 * only the visible window of rows is rendered, so a folder with thousands of entries stays
 * cheap, and subtree scans (is anything inside excluded?) run in the backend only for the
 * included folders you can see.
 */
export default function PlanTreePreview({
  rules,
  onApplyEdit,
  history = [],
  onUndo,
  active = true,
}: Props) {
  // The whole filesystem is the starting point; "Sources only" is a shortcut for a plan whose
  // sources are buried deep.
  const [mode, setMode] = useState<RootMode>("filesystem");
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(storedHeight);
  const [showHidden, setShowHidden] = useState(false);
  const [problems, setProblems] = useState<RuleProblem[]>([]);
  const [editTarget, setEditTarget] = useState<EditTarget | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [revealTarget, setRevealTarget] = useState<string | null>(null);
  // The data lives in refs and a counter forces a re-render: scan results stream in at a high
  // rate and each one would otherwise copy these maps.
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const nodes = useRef(new Map<string, NodeInfo>());
  const verdicts = useRef(new Map<string, NodeVerdict>());
  const scans = useRef(new Map<string, ScanResult>());
  const origins = useRef(new Map<number, RuleOrigin>());

  // Where the tree starts (`/`, or a drive list on Windows) comes from the backend; the OS's
  // spelling of paths decides how the plan's sources are read (`C:\\Users\\me` ↔ `C:/Users/me`).
  const windows = isWindows();
  const [env, setEnv] = useState<TreeEnv | null>(null);
  useEffect(() => {
    treeEnv()
      .then(setEnv)
      .catch(() => setEnv({ windows, roots: [], home: null }));
  }, [windows]);
  const ready = env !== null;

  // What the plan's path-list files add (`--files-from`): read-only sources, expanded by the
  // backend the way restic reads them. Loaded when the list of files changes (and on Refresh).
  const filesFrom = rules.filesFrom ?? [];
  const filesFromKey = JSON.stringify(filesFrom);
  const [listResult, setListResult] = useState<ListFilesResult>(NO_LIST);
  const loadList = useCallback(async () => {
    const files = JSON.parse(filesFromKey) as string[];
    if (files.length === 0) {
      setListResult(NO_LIST);
      return;
    }
    try {
      setListResult(await treeListFiles(files));
    } catch (e) {
      setListResult({ entries: [], problems: [{ what: files.join(", "), error: String(e) }], truncated: false });
    }
  }, [filesFromKey]);
  useEffect(() => {
    if (!ready || !active) return;
    const t = setTimeout(loadList, filesFromKey === "[]" ? 0 : 350);
    return () => clearTimeout(t);
  }, [ready, active, loadList, filesFromKey]);
  const listSources = useMemo(() => listResult.entries.map((e) => e.path), [listResult]);
  /** Where each list-file source came from (the first line that named it). */
  const listOrigin = useMemo(() => {
    const m = new Map<string, ListEntry>();
    for (const e of listResult.entries) if (!m.has(e.path)) m.set(e.path, e);
    return m;
  }, [listResult]);
  const fullRules: TreeRulesInput = { ...rules, listSources, judgeSources: env?.resticJudgesSources ?? false };

  // Rules are debounced: typing a pattern shouldn't rescan on every keystroke.
  const rulesKey = JSON.stringify(fullRules);
  const [activeKey, setActiveKey] = useState(rulesKey);
  useEffect(() => {
    const t = setTimeout(() => setActiveKey(rulesKey), 350);
    return () => clearTimeout(t);
  }, [rulesKey]);
  const activeRules = useMemo(() => JSON.parse(activeKey) as TreeRulesInput, [activeKey]);
  const rulesRef = useRef(activeRules);
  rulesRef.current = activeRules;
  /** The verdicts on screen were computed for the rules being edited right now. */
  const settled = activeKey === rulesKey;

  const generation = useRef(0);
  const inflight = useRef(new Set<string>());
  const latestScan = useRef(0);
  const minScan = useRef(0);
  const scanTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);

  /** The plan's sources in the tree's spelling: as edited now, and as the verdicts on screen know them. */
  const sourcesNow = useMemo(() => new Set(rules.sources.map((s) => toCanonical(s, windows))), [rules.sources, windows]);
  const shownPlanSources = useMemo(
    () => Array.from(new Set(activeRules.sources.map((s) => toCanonical(s, windows)))),
    [activeRules.sources, windows],
  );
  const shownSourceSet = useMemo(() => new Set(shownPlanSources), [shownPlanSources]);
  /** Every source the verdicts on screen know: the plan's own and those from path-list files. */
  const shownSources = useMemo(
    () => Array.from(new Set([...shownPlanSources, ...(activeRules.listSources ?? [])])),
    [shownPlanSources, activeRules.listSources],
  );
  const pinned = useMemo(() => pinnedPaths(shownSources), [shownSources]);

  const roots = useMemo(
    () =>
      mode === "sources"
        ? topLevelSources(shownSources).map((p) => ({
            path: p,
            // Shown the way the OS writes it.
            name: toNative(p, windows),
          }))
        : env && env.roots.length > 0
          ? env.roots.map((r) => ({ path: r.path, name: env.windows ? r.label : `${r.label} (${r.path})` }))
          : [{ path: "/", name: "Computer (/)" }],
    [mode, shownSources, windows, env],
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
        if (!nodes.current.has(child)) {
          nodes.current.set(child, { path: child, name: e.name, kind: e.kind, hidden: e.hidden });
        }
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
      const all = [...rulesRef.current.sources, ...(rulesRef.current.listSources ?? [])];
      for (const s of topLevelSources(all.map((x) => toCanonical(x, windows))).slice(0, 10)) {
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
    // Anything expanded but not yet listed (first open, or new roots). A source that is a file has
    // nothing to list (`listFolder` skips it).
    for (const p of toOpen) await listFolder(p);
  }, [classify, listFolder, mode, roots, windows]);

  // Start (and restart on any change of rules or roots) once the tab is showing.
  useEffect(() => {
    if (!ready || !active) return;
    refresh();
  }, [ready, active, activeKey, mode, refresh]);

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
    if (!ready || !active) return;
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
  }, [ready, active]);

  // ── what to render ────────────────────────────────────────────────────────

  const rows = flattenRows(roots, nodes.current, expanded, {
    showHidden,
    pinned,
    included: (p) => verdicts.current.get(p)?.included,
  });
  const { start, end } = visibleWindow(rows.length, scrollTop, viewH, ROW_H);
  const hidden = showHidden ? null : hiddenSummary(nodes.current, (p) => verdicts.current.get(p)?.included);

  // Scan the included folders in view (plus overscan) that haven't been checked yet. A new scan
  // supersedes the running one in the backend, so everything still unresolved is re-sent.
  const scanKey = `${start}:${end}:${rows.length}:${scans.current.size}:${verdicts.current.size}`;
  useEffect(() => {
    if (!ready || !active) return;
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
  }, [ready, active, scanKey]);

  // ── navigation ────────────────────────────────────────────────────────────

  const toggle = (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
    if (!expanded.has(path)) listFolder(path);
  };

  /** Opens every folder down to `path` and scrolls it into view. */
  const reveal = useCallback(
    async (path: string) => {
      const chain = [...ancestorsOf(path), path];
      for (let i = 0; i < chain.length - 1; i++) {
        const dir = chain[i];
        if (!nodes.current.has(dir)) return;
        setExpanded((prev) => (prev.has(dir) ? prev : new Set(prev).add(dir)));
        await listFolder(dir);
        // A listing already under way (started elsewhere) isn't awaited by `listFolder`.
        for (let n = 0; n < 40 && nodes.current.get(dir)?.loading; n++) await new Promise((r) => setTimeout(r, 50));
        if (!nodes.current.has(chain[i + 1])) return;
      }
      setRevealTarget(path);
      setTimeout(() => setRevealTarget((cur) => (cur === path ? null : cur)), 3000);
    },
    [listFolder],
  );

  // Scrolls to the row being revealed once it is part of the flattened tree.
  useEffect(() => {
    if (!revealTarget) return;
    const idx = rows.findIndex((r) => r.type === "node" && r.path === revealTarget);
    if (idx < 0) return;
    const el = viewportRef.current;
    if (el) el.scrollTop = Math.max(0, idx * ROW_H - viewH / 3);
    setRevealTarget(null);
    // `rows` is rebuilt on every render, so this also re-checks after each listing arrives.
  }, [revealTarget, rows, viewH]);

  const startResize = (e: React.PointerEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = viewH;
    let latest = startH;
    const move = (ev: PointerEvent) => {
      latest = clampHeight(startH + ev.clientY - startY);
      setViewH(latest);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try {
        localStorage.setItem(HEIGHT_KEY, String(latest));
      } catch {
        /* a per-viewer convenience only */
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  const resizeByKey = (e: React.KeyboardEvent) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const h = clampHeight(viewH + (e.key === "ArrowDown" ? 40 : -40));
    setViewH(h);
    try {
      localStorage.setItem(HEIGHT_KEY, String(h));
    } catch {
      /* a per-viewer convenience only */
    }
  };

  // ── editing ───────────────────────────────────────────────────────────────

  const openEdit = (path: string, kind: SourceKind) => {
    setEditTarget({
      path,
      name: baseName(path),
      kind,
      verdict: verdicts.current.get(path),
      isSource: sourcesNow.has(path),
    });
  };

  /**
   * A click on a row's box. Sources and items outside every source change in one step (the edit is
   * computed and verified first, and can be undone); anything that needs a choice, or that would
   * swallow existing sources, goes through the dialog, which shows exactly what will change.
   */
  const onBox = async (path: string, kind: SourceKind) => {
    // Wait for the verdicts to catch up with the last edit, or the click would act on stale state.
    if (!onApplyEdit || !settled || busy) return;
    const origin = listOrigin.get(path);
    if (origin && !sourcesNow.has(path)) {
      // Listed in a path-list file: the file is the source of truth, so the change is made there.
      setActionError(
        `${baseName(path)} is on line ${origin.line} of ${origin.file}. To stop backing it up, remove or change that line in the file (then Refresh).`,
      );
      return;
    }
    const click = boxClick(sourcesNow.has(path), verdicts.current.get(path));
    if (click === "dialog") {
      openEdit(path, kind);
      return;
    }
    setActionError(null);
    setBusy(true);
    try {
      const outcome = await treePlanEdit(
        fullRules,
        click === "removeSource"
          ? { kind: "exclude", path, pattern: escapeGlob(path, windows) }
          : { kind: "include", path, scope: "item" },
      );
      if (click === "addSource" && (outcome.removedSources.length > 0 || outcome.addedRules.length > 0)) {
        openEdit(path, kind);
      } else {
        onApplyEdit(outcome);
      }
    } catch (e) {
      setActionError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const reload = () => {
    nodes.current.clear();
    loadList();
    refresh();
  };

  const emptySources = mode === "sources" && roots.length === 0;
  const nothingPicked = rules.sources.length === 0 && filesFrom.length === 0;
  const listProblems = listResult.problems;
  const visible = rows.slice(start, end);
  const latest = history[history.length - 1];

  return (
    <div className={CARD}>
      <div className="mb-4">
        <h2 className="text-sm font-medium text-gray-300">Files and folders</h2>
        <p className="text-xs text-gray-500 mt-0.5">
          Pick what to back up: click the box next to a folder or file. Inside a folder you picked, click a box to leave
          something out — or to bring it back.
        </p>
        {env?.resticJudgesSources && (
          <p className="text-xs text-gray-500 mt-1">
            Your restic is older than 0.19, which also applies the exclude rules to a folder or file you pick directly: a
            rule that matches it leaves it out.
          </p>
        )}
      </div>

      <div className="flex items-center justify-between gap-3 mb-2 flex-wrap">
        <div className="flex items-center gap-2">
          <div className="flex rounded-lg overflow-hidden border border-gray-700">
            {(["filesystem", "sources"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`px-3 py-1 text-xs font-medium transition-colors ${
                  mode === m ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"
                }`}
              >
                {m === "sources" ? "Sources only" : "Whole filesystem"}
              </button>
            ))}
          </div>
          {mode === "filesystem" && env?.home && (
            <button
              type="button"
              onClick={() => reveal(env.home!)}
              className="px-2.5 py-1 text-xs font-medium rounded-lg border border-gray-700 bg-gray-800 text-gray-400 hover:text-gray-50 transition-colors"
              title="Jump to your home folder"
            >
              Home
            </button>
          )}
        </div>

        <div className="flex items-center gap-4 text-xs text-gray-500">
          <span className="inline-flex items-center gap-2 select-none">
            <button
              type="button"
              role="switch"
              aria-checked={showHidden}
              aria-label="Show hidden files"
              onClick={() => setShowHidden((v) => !v)}
              className={`relative inline-flex h-4 w-7 items-center rounded-full transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 ${
                showHidden ? "bg-blue-600" : "bg-gray-700"
              }`}
            >
              <span
                className={`inline-block h-3 w-3 rounded-full bg-white shadow transition-transform ${
                  showHidden ? "translate-x-3.5" : "translate-x-0.5"
                }`}
              />
            </button>
            <span>Show hidden files</span>
            {hidden && hidden.backedUp > 0 && (
              <Tooltip
                content={
                  <p>
                    Hidden files are backed up too — {hidden.backedUp.toLocaleString()} hidden{" "}
                    {hidden.backedUp === 1 ? "item" : "items"} in the folders you&apos;ve opened. Turn this on to see{" "}
                    {hidden.backedUp === 1 ? "it" : "them"}.
                  </p>
                }
              >
                <span
                  className="inline-flex items-center justify-center w-4 h-4 rounded-full bg-amber-900/40 border border-amber-500/50 text-amber-400 text-[11px] font-bold leading-none cursor-help"
                  aria-label="Hidden files are backed up too"
                >
                  !
                </span>
              </Tooltip>
            )}
          </span>
          <button type="button" onClick={reload} className="text-gray-400 hover:text-gray-50 underline-offset-2 hover:underline">
            Refresh
          </button>
          <button type="button" onClick={() => setExportOpen(true)} className="text-gray-400 hover:text-gray-50 underline-offset-2 hover:underline">
            Use with restic CLI…
          </button>
        </div>
      </div>

      <div className="flex items-center gap-4 mb-3 text-xs text-gray-500 flex-wrap">
        <span className="inline-flex items-center gap-1.5"><StateBox state="included" /> Backed up</span>
        <span className="inline-flex items-center gap-1.5"><StateBox state="partial" /> Backed up, some items left out</span>
        <span className="inline-flex items-center gap-1.5"><StateBox state="contains" /> Holds a source</span>
        <span className="inline-flex items-center gap-1.5"><StateBox state="excluded" /> Not backed up</span>
      </div>

      {history.length > 0 && latest && onUndo && (
        <p className="mb-3 text-xs text-gray-400" role="status">
          Updated your plan: {latest.summary}.{" "}
          <button type="button" onClick={onUndo} className="text-blue-400 hover:text-blue-300 underline-offset-2 hover:underline">
            Undo
          </button>
          {history.length > 1 && (
            <span className="text-gray-500"> · {history.length - 1} earlier {history.length === 2 ? "change" : "changes"} can be undone after this</span>
          )}
        </p>
      )}

      {actionError && (
        <p className="mb-3 text-xs text-amber-400 whitespace-pre-wrap break-words" role="alert">
          {actionError}
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

      {nothingPicked && !emptySources && (
        <p className="mb-3 text-xs text-gray-400">Nothing is selected yet. Open a folder and click its box to back it up.</p>
      )}
      {filesFrom.length > 0 && (
        <div className="mb-3 text-xs text-gray-500">
          <p>
            {listResult.entries.length > 0
              ? `${listResult.entries.length.toLocaleString()} ${listResult.entries.length === 1 ? "entry" : "entries"} from your path-list ${filesFrom.length === 1 ? "file" : "files"} ${listResult.entries.length === 1 ? "is" : "are"} marked “From list”. Change them by editing the file, then Refresh.`
              : "Your path-list files don't add anything to the tree."}
          </p>
          {listProblems.length > 0 && (
            <ul className="mt-1 space-y-1">
              {listProblems.slice(0, MAX_LIST_PROBLEMS).map((p, i) => (
                <li key={i} className="flex items-start gap-1.5 text-amber-400">
                  <WarningIcon className="w-3.5 h-3.5 flex-shrink-0 mt-px" />
                  <span><span className="font-mono break-all">{p.what}</span>: {p.error}</span>
                </li>
              ))}
              {listProblems.length > MAX_LIST_PROBLEMS && (
                <li className="pl-5">…and {listProblems.length - MAX_LIST_PROBLEMS} more.</li>
              )}
            </ul>
          )}
        </div>
      )}

      {emptySources ? (
        <p className="text-sm text-gray-500 text-center py-6">
          No sources yet. Switch to <span className="text-gray-300">Whole filesystem</span> and click the box next to a folder or file to back it up.
        </p>
      ) : (
        <>
          <div
            ref={viewportRef}
            role="tree"
            aria-label="Files and folders in this backup plan"
            className="relative overflow-auto rounded-t-lg border border-gray-800 bg-gray-950/40"
            style={{ height: viewH }}
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
                      {row.action === "showHidden" && (
                        <button
                          type="button"
                          onClick={() => setShowHidden(true)}
                          className="ml-2 text-blue-400 hover:text-blue-300 underline-offset-2 hover:underline"
                        >
                          Show
                        </button>
                      )}
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
                const isSource = shownSourceSet.has(row.path);
                // From a path-list file (and not also one of the plan's own sources).
                const fromList = !isSource ? listOrigin.get(row.path) : undefined;
                if (fromList) {
                  lines.unshift(
                    fromList.pattern === row.path
                      ? `Listed on line ${fromList.line} of ${fromList.file}.`
                      : `Matched by \`${fromList.pattern}\` on line ${fromList.line} of ${fromList.file}.`,
                  );
                }
                const click = boxClick(isSource, verdict);
                return (
                  <div
                    key={row.path}
                    role="treeitem"
                    aria-level={row.depth + 1}
                    aria-expanded={isDir ? row.expanded : undefined}
                    aria-label={`${row.name}: ${missing ? "missing" : STATE_LABEL[state]}${isSource ? " (backup source)" : fromList ? " (from a path-list file)" : ""}`}
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
                        onClick={() => onBox(row.path, row.kind)}
                        className="rounded-[3px] hover:ring-2 hover:ring-blue-500/40 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 flex-shrink-0"
                        title={fromList ? "From a path-list file — edit the file to change it" : boxTitle(click, verdict?.included, row.kind)}
                        aria-label={`${row.name}: ${(fromList ? "from a path-list file" : boxTitle(click, verdict?.included, row.kind)).toLowerCase()}`}
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
                    {isSource && <SourcePill />}
                    {fromList && <SourcePill kind="list" />}
                    {missing && <span className="text-xs text-amber-400 ml-1">doesn&apos;t exist</span>}
                    {state === "unverified" && <span className="text-xs text-gray-500 ml-1">not fully checked</span>}
                    {state === "partial" && <span className="text-xs text-gray-500 ml-1">some items excluded</span>}
                    {state === "contains" && <span className="text-xs text-gray-500 ml-1">holds a source</span>}
                  </div>
                );
              })}
            </div>
          </div>
          <div
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize the file tree"
            aria-valuemin={MIN_VIEW_H}
            aria-valuemax={MAX_VIEW_H}
            aria-valuenow={viewH}
            tabIndex={0}
            onPointerDown={startResize}
            onKeyDown={resizeByKey}
            title="Drag to resize"
            className="group flex items-center justify-center h-3 rounded-b-lg border border-t-0 border-gray-800 bg-gray-900 cursor-row-resize touch-none focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
          >
            <span className="block w-8 h-[3px] rounded bg-gray-700 group-hover:bg-gray-500" />
          </div>
        </>
      )}
      <p className="text-xs text-gray-500 mt-2">
        Computed with the same rules restic applies. Hover a name to see why it is or isn&apos;t backed up.
      </p>

      {onApplyEdit && (
        <PlanTreeEditDialog target={editTarget} rules={fullRules} onClose={() => setEditTarget(null)} onApply={onApplyEdit} />
      )}
      <PlanTreeExportDialog open={exportOpen} rules={fullRules} onClose={() => setExportOpen(false)} />
    </div>
  );
}
