import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { cancelIndexBatch, getActiveIndexBatch, getSnapshotIndexStatus, indexSnapshotsBatch, listSnapshots, searchRepoFiles } from "../lib/invoke";
import { INDEX_BATCH_ALREADY_ACTIVE_ERROR, type ActiveIndexBatchStatus, type RepoFileHit, type Snapshot, type TaskEvent } from "../lib/types";
import { formatDate, formatSize } from "../lib/format";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import Spinner from "../components/Spinner";

type IndexState = "loading" | "not_indexed" | "ready";

const FileIcon = ({ type }: { type: string }) => {
  if (type === "dir") {
    return (
      <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="currentColor" viewBox="0 0 20 20">
        <path d="M2 6a2 2 0 012-2h4l2 2h6a2 2 0 012 2v6a2 2 0 01-2 2H4a2 2 0 01-2-2V6z" />
      </svg>
    );
  }
  return (
    <svg className="w-4 h-4 text-gray-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
        d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
    </svg>
  );
};

// Build the path stack for BrowsePage navigation so pressing Back works naturally.
function browseTarget(entry: RepoFileHit): { path: string | undefined; stack: string[] } {
  const parts = entry.path.split("/").filter(Boolean);
  const targetParts = entry.type === "dir" ? parts : parts.slice(0, -1);
  const path = targetParts.length ? "/" + targetParts.join("/") : undefined;
  const stack: string[] = [""];
  for (let i = 0; i < targetParts.length - 1; i++) {
    stack.push("/" + targetParts.slice(0, i + 1).join("/"));
  }
  return { path, stack };
}

export default function RepoSearchPage() {
  const { repoId } = useParams<{ repoId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const locationState = location.state as { restoredQuery?: string; restoredResults?: RepoFileHit[] } | null;

  const [indexState, setIndexState] = useState<IndexState>("loading");
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [indexStatus, setIndexStatus] = useState<Record<string, string>>({});
  const [indexAllError, setIndexAllError] = useState("");
  const [indexAllOpen, setIndexAllOpen] = useState(false);
  const [indexAllTargets, setIndexAllTargets] = useState<string[]>([]);
  const [indexAllCompleted, setIndexAllCompleted] = useState<Set<string>>(new Set());
  const [indexAllStopped, setIndexAllStopped] = useState(false);
  const indexAllTargetsRef = useRef<string[]>([]);
  // The one piece of state for this repo's "Index All" batch — deliberately a single
  // discriminated union rather than three separately-managed booleans/strings (an earlier
  // version had `batchOperationId`/`batchQueued`/`indexAllBatchActive` as independent
  // `useState`s, always updated together across three code paths — mount restore, the task
  // listener, and handleIndexAll — with no guarantee they stayed consistent). The four kinds
  // are the only ones that are ever real:
  //   - "idle": no batch for this repo, right of way to start a fresh one.
  //   - "starting": handleIndexAll just called indexSnapshotsBatch; no operationId yet (the
  //     "pending" task event carrying it is an async round-trip behind it), so Stop has
  //     nothing to target — same window the old `batchOperationId === null` covered.
  //   - "queued": registered and cancellable (via operationId), but hasn't won its turn on
  //     the backend's batch_turn mutex yet — see IndexHandle::batch_turn, cache.rs.
  //   - "running": won its turn, actually indexing.
  // Deliberately independent of `indexAllOpen` — the modal can be dismissed while a batch
  // keeps running/queued in the background (see the module doc comment above). The actual
  // duplicate-request guard lives in the backend (index_snapshots_batch rejects a second call
  // for a repo that already has one queued/running, browse.rs) — this state isn't what
  // prevents that. It's what lets handleIndexAll tell "start a fresh batch" apart from "one
  // already exists, just show its current status" so clicking "Index All" again doesn't reset
  // the progress bar to 0 or lose track of the real batch's operationId (breaking Stop) for a
  // click the backend was always going to reject anyway.
  type BatchState =
    | { kind: "idle" }
    | { kind: "starting" }
    | { kind: "queued"; operationId: string }
    | { kind: "running"; operationId: string };
  const [batchState, setBatchState] = useState<BatchState>({ kind: "idle" });
  const indexAllBatchActive = batchState.kind !== "idle";
  const batchQueued = batchState.kind === "queued";
  const batchOperationId = batchState.kind === "queued" || batchState.kind === "running" ? batchState.operationId : null;

  const [query, setQuery] = useState(locationState?.restoredQuery ?? "");
  const [results, setResults] = useState<RepoFileHit[]>(locationState?.restoredResults ?? []);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [searched, setSearched] = useState(locationState?.restoredQuery != null && locationState.restoredQuery.trim().length > 0);

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const snapshotById = useMemo(() => {
    const map = new Map<string, Snapshot>();
    for (const s of snapshots) map.set(s.id, s);
    return map;
  }, [snapshots]);

  const indexedCount = useMemo(
    () => snapshots.filter((s) => indexStatus[s.id] === "complete").length,
    [snapshots, indexStatus]
  );

  const indexAllTotal = indexAllTargets.length;
  const indexAllDoneCount = indexAllCompleted.size;
  const indexAllFailedCount = useMemo(
    () => indexAllTargets.filter((id) => indexAllCompleted.has(id) && indexStatus[id] !== "complete").length,
    [indexAllTargets, indexAllCompleted, indexStatus]
  );

  useEffect(() => {
    indexAllTargetsRef.current = indexAllTargets;
  }, [indexAllTargets]);

  // Adopts a batch reported by getActiveIndexBatch as this page's own tracked state — shared
  // by the mount-restore effect below and handleIndexAll's error path (a request rejected
  // because the backend already has a batch for this repo). `statusMap` is passed in rather
  // than read from `indexStatus` state so the mount-restore call site can use the just-fetched
  // local value instead of racing the `indexStatus` state update.
  const adoptActiveBatchStatus = (status: ActiveIndexBatchStatus, statusMap: Record<string, string>) => {
    const completed = new Set(status.targetIds.filter((id) => statusMap[id] === "complete"));
    setIndexAllTargets(status.targetIds);
    setIndexAllCompleted(completed);
    setBatchState({ kind: status.started ? "running" : "queued", operationId: status.operationId });
  };

  useEffect(() => {
    return () => { if (debounceRef.current) clearTimeout(debounceRef.current); };
  }, []);

  useEffect(() => {
    if (!repoId) return;
    let cancelled = false;
    Promise.all([listSnapshots(repoId), getSnapshotIndexStatus(repoId)])
      .then(([snaps, statusMap]) => {
        if (cancelled) return;
        setSnapshots(snaps);
        setIndexStatus(statusMap);
        const anyComplete = snaps.some((s) => statusMap[s.id] === "complete");
        setIndexState(anyComplete ? "ready" : "not_indexed");
        if (anyComplete) setTimeout(() => inputRef.current?.focus(), 50);

        // Restores queued/running batch state for this repo by asking the backend directly,
        // rather than relying solely on live `task` events observed since this component
        // mounted — those alone would miss a batch that was already queued or running before
        // the user navigated here (e.g. started from this same page, then Back/Forward, or
        // started via a different repo's page while this one has a batch outstanding). The
        // backend (browse.rs's index_snapshots_batch) is the actual source of truth and
        // rejects a duplicate request regardless of what this component believes; this query
        // just lets the UI reflect that truth immediately instead of only learning about it
        // from the next live task event.
        //
        // Chained here (using `snaps`/`statusMap` directly) rather than as a second,
        // independent effect: a restored batch's `targetIds` need to be cross-referenced
        // against index status to seed indexAllTargets/indexAllCompleted accurately, and
        // reading that from this closure's local variables avoids racing against the `snaps`/
        // `statusMap` *state* (set just above, but not necessarily flushed by the time a
        // separate effect's own async call resolved) — a real bug an earlier version had:
        // indexAllTargets was never populated on restore at all, so a recovered batch's modal
        // rendered "Indexing complete — 0 of 0 snapshots" instead of its real progress.
        return getActiveIndexBatch(repoId).then((status) => {
          if (cancelled || !status) return;
          adoptActiveBatchStatus(status, statusMap);
        }).catch(() => {});
      })
      .catch(() => {
        if (!cancelled) setIndexState("not_indexed");
      });
    return () => { cancelled = true; };
  }, [repoId]);

  // Guards against out-of-order responses: the backend query can take a second
  // or more, so a burst of keystrokes (or a task-event-triggered re-search) can
  // have several searches in flight at once. Only the response matching the
  // latest call is allowed to update state.
  const searchSeqRef = useRef(0);

  const runSearch = useCallback(async (q: string) => {
    const seq = ++searchSeqRef.current;
    if (!repoId || !q.trim()) {
      setResults([]);
      setSearched(false);
      return;
    }
    setSearching(true);
    setSearchError("");
    try {
      const data = await searchRepoFiles(repoId, q);
      if (seq !== searchSeqRef.current) return;
      setResults(data);
      setSearched(true);
    } catch (err: any) {
      if (seq !== searchSeqRef.current) return;
      setSearchError(String(err));
    } finally {
      if (seq === searchSeqRef.current) setSearching(false);
    }
  }, [repoId]);

  // Live updates while indexing (both the per-row listener below and "Index All")
  // rely on this to flip not_indexed → ready and to re-run the current query so
  // newly-indexed snapshots' files appear without the user retyping.
  useEffect(() => {
    if (!repoId) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<TaskEvent>("task", (e) => {
      const t = e.payload;
      if (t.kind !== "index" || t.repoId !== repoId) return;
      // The batch-level op (no targetId) — capture its operationId so Stop can target this
      // page's own batch specifically, independent of any other batch running elsewhere.
      // Captured on "pending" (not just "started") so Stop works while the batch is still
      // queued waiting its turn, not only once it's actually running.
      if (!t.targetId) {
        if (t.origin !== "manual") return;
        if (t.phase === "pending") {
          setBatchState({ kind: "queued", operationId: t.operationId });
        } else if (t.phase === "started") {
          setBatchState({ kind: "running", operationId: t.operationId });
        } else if (t.phase === "finished" || t.phase === "failed" || t.phase === "cancelled") {
          // Lets handleIndexAll treat the next click as a fresh start again — see
          // BatchState's doc comment. Not gated on matching operationId: the backend only
          // ever allows one batch per repo at a time (index_snapshots_batch rejects a
          // duplicate, browse.rs), so any terminal batch-level event for this repo is
          // necessarily this page's own batch finishing.
          setBatchState({ kind: "idle" });
        }
        return;
      }
      if (t.phase !== "finished" && t.phase !== "failed") return;
      const snapshotId = t.targetId;
      const success = t.phase === "finished";
      setIndexStatus((prev) => ({ ...prev, [snapshotId]: success ? "complete" : "pending" }));
      if (indexAllTargetsRef.current.includes(snapshotId)) {
        setIndexAllCompleted((prev) => {
          if (prev.has(snapshotId)) return prev;
          const next = new Set(prev);
          next.add(snapshotId);
          return next;
        });
      }
      if (success) {
        setIndexState("ready");
        if (query.trim()) runSearch(query);
      }
    }).then((u) => {
      if (cancelled) { u(); return; }
      unlisten = u;
    });
    return () => { cancelled = true; unlisten?.(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repoId, query]);

  const handleIndexAll = async () => {
    if (!repoId) return;
    // A batch is already queued or running for this repo (see BatchState's doc comment) —
    // don't reset local progress state or call the backend again (it would just be rejected
    // per index_snapshots_batch's dedup guard, browse.rs). Instead, just show the modal
    // reflecting whatever's already tracked, so Stop keeps targeting the real batch and the
    // progress bar doesn't jump back to 0.
    if (indexAllBatchActive) {
      setIndexAllOpen(true);
      return;
    }
    const targets = snapshots
      .filter((s) => indexStatus[s.id] !== "complete" && indexStatus[s.id] !== "in_progress")
      .map((s) => s.id);
    if (targets.length === 0) return;
    setIndexAllError("");
    setIndexAllCompleted(new Set());
    setIndexAllStopped(false);
    setIndexAllTargets(targets);
    setIndexAllOpen(true);
    // No operationId yet — the "pending" task event carrying it is an async round-trip
    // behind this call (see BatchState's doc comment on the "starting" kind).
    setBatchState({ kind: "starting" });
    try {
      // Indexed sequentially, one snapshot at a time, by the backend — bounds
      // memory to a single snapshot's file list and pauses the background
      // auto-indexer for the duration. Progress arrives via the task listener below.
      await indexSnapshotsBatch(repoId, targets);
    } catch (err) {
      if (String(err) === INDEX_BATCH_ALREADY_ACTIVE_ERROR) {
        // The backend rejected because it already has a batch for this repo — this
        // component believed otherwise (a narrow race; see index_snapshots_batch's doc
        // comment, browse.rs). Resync to the real batch's state instead of showing a
        // scary "failed to start" for a request that was never actually attempted.
        try {
          const status = await getActiveIndexBatch(repoId);
          if (status) {
            adoptActiveBatchStatus(status, indexStatus);
          } else {
            setBatchState({ kind: "idle" });
          }
        } catch {
          setBatchState({ kind: "idle" });
        }
        return;
      }
      setIndexAllError("Failed to start indexing.");
      setBatchState({ kind: "idle" });
    }
  };

  const handleStopIndexAll = async () => {
    // Guarded by the Stop button's own `disabled` prop below, but re-checked here too since
    // this is also reachable while disabled=true briefly re-renders (belt and suspenders).
    if (!batchOperationId) return;
    try {
      await cancelIndexBatch(batchOperationId);
      setIndexAllStopped(true);
    } catch {
      // best-effort; batch will keep running if this fails
    }
  };

  const handleQueryChange = (value: string) => {
    setQuery(value);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (!value.trim()) {
      searchSeqRef.current++;
      setResults([]);
      setSearched(false);
      return;
    }
    debounceRef.current = setTimeout(() => runSearch(value), 300);
  };

  const handleResultClick = (entry: RepoFileHit) => {
    const { path, stack } = browseTarget(entry);
    // Persist query+results into the current history entry so navigate(-1) from BrowsePage restores them.
    window.history.replaceState(
      { ...window.history.state, usr: { ...locationState, restoredQuery: query, restoredResults: results } },
      ''
    );
    navigate(`/snapshots/${repoId}/${entry.snapshotId}/browse`, {
      state: { snapshot: snapshotById.get(entry.snapshotId), initialPath: path, initialPathStack: stack, fromSearch: true },
    });
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={() => navigate(`/snapshots/${repoId}`)}>
            ← Snapshots
          </Button>
        </div>
        <h1 className="text-xl font-semibold text-gray-100 mt-3">Search Repository</h1>
        <p className="text-sm text-gray-500 mt-0.5">
          Search files across every indexed snapshot in this repository. Each match only shows the most recent snapshot that contains it.
        </p>
      </div>

      {indexState === "loading" && (
        <div className="flex items-center gap-3 text-sm text-gray-500 py-12 justify-center">
          <Spinner className="w-5 h-5 text-blue-400" />
          Checking index status…
        </div>
      )}

      {indexState === "not_indexed" && (
        <div className="flex flex-col items-center justify-center py-16 gap-5 text-center max-w-sm mx-auto">
          <div className="p-4 rounded-full bg-gray-800">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.5} className="w-10 h-10 text-gray-400">
              <path strokeLinecap="round" strokeLinejoin="round" d="m21 21-5.197-5.197m0 0A7.5 7.5 0 1 0 5.196 15.803m10.607 0A7.5 7.5 0 0 0 5.196 15.803" />
            </svg>
          </div>
          <div>
            <p className="text-gray-100 font-medium mb-1">No snapshots indexed yet</p>
            <p className="text-sm text-gray-500">
              Repository search requires at least one indexed snapshot. Indexing reads each snapshot's file list once and stores it locally — it may take a moment for large repositories.
            </p>
          </div>
          {indexAllError && (
            <p className="text-sm text-red-300">{indexAllError}</p>
          )}
          <Button onClick={handleIndexAll} disabled={snapshots.length === 0}>
            Index All Snapshots
          </Button>
        </div>
      )}

      {indexState === "ready" && (
        <div>
          {indexedCount < snapshots.length && (
            <div className="flex items-center justify-between gap-3 mb-4 px-4 py-2.5 rounded-lg bg-gray-900/60 border border-gray-800 text-sm">
              <span className="text-gray-400">
                Searching {indexedCount} of {snapshots.length} snapshots
                {indexAllBatchActive && ` · ${batchQueued ? "queued" : "indexing"}…`}
              </span>
              <Button variant="ghost" size="sm" onClick={handleIndexAll}>
                Index All
              </Button>
            </div>
          )}
          {indexAllError && (
            <p className="text-sm text-red-300 mb-3">{indexAllError}</p>
          )}

          <div className="mb-5">
            <Input
              ref={inputRef}
              placeholder="Search by file name or path…"
              value={query}
              onChange={(e) => handleQueryChange(e.target.value)}
              onClear={() => handleQueryChange("")}
              className="w-full"
            />
          </div>

          {searching && (
            <div className="flex items-center gap-2 text-sm text-gray-500 py-8 justify-center">
              <Spinner className="w-4 h-4 text-blue-400" />
              Searching…
            </div>
          )}

          {searchError && !searching && (
            <p className="text-sm text-red-300 mb-4">{searchError}</p>
          )}

          {!searching && !query.trim() && (
            <div className="flex flex-col items-center gap-2 py-16 text-gray-600">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.2} className="w-12 h-12">
                <path strokeLinecap="round" strokeLinejoin="round" d="m21 21-5.197-5.197m0 0A7.5 7.5 0 1 0 5.196 15.803m10.607 0A7.5 7.5 0 0 0 5.196 15.803" />
              </svg>
              <p className="text-sm">Type to search across all indexed snapshots in this repository</p>
            </div>
          )}

          {!searching && searched && query.trim() && results.length === 0 && (
            <div className="flex flex-col items-center gap-2 py-16 text-gray-600">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-8 h-8">
                <path fillRule="evenodd" d="M9 3.5a5.5 5.5 0 1 0 0 11 5.5 5.5 0 0 0 0-11ZM2 9a7 7 0 1 1 12.452 4.391l3.328 3.329a.75.75 0 1 1-1.06 1.06l-3.329-3.328A7 7 0 0 1 2 9Z" clipRule="evenodd" />
              </svg>
              <p className="text-sm">No files matching <span className="text-gray-400 font-mono">"{query}"</span></p>
            </div>
          )}

          {!searching && results.length > 0 && (
            <div>
              <p className="text-xs text-gray-500 mb-3">
                {results.length === 200 ? "Showing first 200 matches" : `${results.length} result${results.length !== 1 ? "s" : ""}`}
              </p>
              <div className="rounded-xl border border-gray-800 overflow-hidden">
                <div className="divide-y divide-gray-800">
                  {results.map((entry) => {
                    const dirPart = entry.path.substring(0, entry.path.lastIndexOf("/") + 1);
                    return (
                      <button
                        key={entry.path}
                        className="w-full flex items-center gap-3 px-4 py-3 hover:bg-gray-900/50 transition-colors text-left group"
                        onClick={() => handleResultClick(entry)}
                        title={`Open in browser: ${entry.path}`}
                      >
                        <FileIcon type={entry.type} />
                        <div className="flex-1 min-w-0">
                          <div className="flex items-baseline gap-1 min-w-0">
                            <span className="text-xs text-gray-600 font-mono truncate shrink-0 max-w-[50%]">{dirPart}</span>
                            <span className="text-sm text-gray-200 font-medium truncate">{entry.name}</span>
                          </div>
                        </div>
                        <div className="flex items-center gap-4 shrink-0 text-xs text-gray-600">
                          <span
                            className="font-mono w-20 shrink-0 text-center px-1.5 py-0.5 rounded bg-gray-800 text-gray-400 whitespace-nowrap"
                            title="Newest snapshot containing this file"
                          >
                            {entry.snapshotShortId}
                          </span>
                          <span className="w-16 shrink-0 text-right tabular-nums whitespace-nowrap">{formatSize(entry.size)}</span>
                          <span className="hidden sm:inline w-40 shrink-0 text-right tabular-nums whitespace-nowrap">
                            {entry.mtime ? formatDate(entry.mtime) : "—"}
                          </span>
                          <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4 text-gray-700 group-hover:text-gray-400 transition-colors flex-shrink-0">
                            <path fillRule="evenodd" d="M8.22 5.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L11.94 10 8.22 6.28a.75.75 0 0 1 0-1.06Z" clipRule="evenodd" />
                          </svg>
                        </div>
                      </button>
                    );
                  })}
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      <Modal
        title="Index All Snapshots"
        open={indexAllOpen}
        onClose={() => setIndexAllOpen(false)}
      >
        {indexAllStopped && indexAllDoneCount < indexAllTotal ? (
          <div className="py-2">
            <p className="text-sm text-gray-300 mb-3">
              Stopped after {indexAllDoneCount} of {indexAllTotal} snapshots.
            </p>
            <p className="text-xs text-gray-600 mb-4">
              The remaining snapshots were not indexed. You can resume later from this page.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : indexAllDoneCount < indexAllTotal ? (
          <div className="py-2">
            {batchQueued ? (
              <>
                <p className="text-sm text-gray-300 mb-3">
                  Queued — waiting for other indexing to finish…
                </p>
                <p className="text-xs text-gray-600 mb-4">
                  Only one repository is indexed at a time. This batch will start automatically
                  as soon as the one ahead of it finishes.
                </p>
              </>
            ) : (
              <>
                <p className="text-sm text-gray-300 mb-3">
                  Indexing {indexAllDoneCount} of {indexAllTotal} snapshots…
                </p>
                <div className="w-full bg-gray-800 rounded-full h-1.5 mb-4">
                  <div
                    className="bg-blue-500 h-1.5 rounded-full transition-all"
                    style={{ width: `${(indexAllDoneCount / indexAllTotal) * 100}%` }}
                  />
                </div>
                <p className="text-xs text-gray-600 mb-4">
                  Snapshots are indexed one at a time. You can close this and keep browsing —
                  indexing continues in the background.
                </p>
              </>
            )}
            <div className="flex justify-end gap-2">
              <Button
                variant="secondary"
                onClick={handleStopIndexAll}
                disabled={!batchOperationId}
                title={batchOperationId ? undefined : "Starting…"}
              >
                Stop
              </Button>
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        ) : (
          <div className="py-2">
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Indexing complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Indexed {indexAllTotal - indexAllFailedCount} of {indexAllTotal} snapshot{indexAllTotal !== 1 ? "s" : ""}.
              {indexAllFailedCount > 0 && ` ${indexAllFailedCount} failed and can be retried individually from the Snapshots page.`}
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setIndexAllOpen(false)}>Close</Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
