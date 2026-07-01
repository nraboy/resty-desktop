import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { getSnapshotIndexStatus, indexSnapshot, listSnapshots, searchRepoFiles } from "../lib/invoke";
import type { RepoFileHit, Snapshot } from "../lib/types";
import { formatDate, formatSize } from "../lib/format";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";

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
  const indexAllTargetsRef = useRef<string[]>([]);

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
  const indexAllInProgress = indexAllOpen && indexAllTotal > 0 && indexAllDoneCount < indexAllTotal;
  const indexAllFailedCount = useMemo(
    () => indexAllTargets.filter((id) => indexAllCompleted.has(id) && indexStatus[id] !== "complete").length,
    [indexAllTargets, indexAllCompleted, indexStatus]
  );

  useEffect(() => {
    indexAllTargetsRef.current = indexAllTargets;
  }, [indexAllTargets]);

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
      })
      .catch(() => {
        if (!cancelled) setIndexState("not_indexed");
      });
    return () => { cancelled = true; };
  }, [repoId]);

  const runSearch = useCallback(async (q: string) => {
    if (!repoId || !q.trim()) {
      setResults([]);
      setSearched(false);
      return;
    }
    setSearching(true);
    setSearchError("");
    try {
      const data = await searchRepoFiles(repoId, q);
      setResults(data);
      setSearched(true);
    } catch (err: any) {
      setSearchError(String(err));
    } finally {
      setSearching(false);
    }
  }, [repoId]);

  // Live updates while indexing (both the per-row listener below and "Index All")
  // rely on this to flip not_indexed → ready and to re-run the current query so
  // newly-indexed snapshots' files appear without the user retyping.
  useEffect(() => {
    if (!repoId) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<{ snapshotId: string; repoId: string; success: boolean }>("index:done", (e) => {
      if (e.payload.repoId !== repoId) return;
      setIndexStatus((prev) => ({ ...prev, [e.payload.snapshotId]: e.payload.success ? "complete" : "pending" }));
      if (indexAllTargetsRef.current.includes(e.payload.snapshotId)) {
        setIndexAllCompleted((prev) => {
          if (prev.has(e.payload.snapshotId)) return prev;
          const next = new Set(prev);
          next.add(e.payload.snapshotId);
          return next;
        });
      }
      if (e.payload.success) {
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
    const targets = snapshots
      .filter((s) => indexStatus[s.id] !== "complete" && indexStatus[s.id] !== "in_progress")
      .map((s) => s.id);
    if (targets.length === 0) return;
    setIndexAllError("");
    setIndexAllCompleted(new Set());
    setIndexAllTargets(targets);
    setIndexAllOpen(true);
    try {
      for (const id of targets) {
        await indexSnapshot(repoId, id);
      }
    } catch {
      setIndexAllError("Failed to start indexing for some snapshots.");
    }
  };

  const handleQueryChange = (value: string) => {
    setQuery(value);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (!value.trim()) {
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
          Search files across every indexed snapshot in this repository.
        </p>
      </div>

      {indexState === "loading" && (
        <div className="flex items-center gap-3 text-sm text-gray-500 py-12 justify-center">
          <svg className="animate-spin w-5 h-5 text-blue-400" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
            <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
            <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
          </svg>
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
          <Button onClick={handleIndexAll} disabled={indexAllInProgress || snapshots.length === 0}>
            {indexAllInProgress ? "Indexing…" : "Index All Snapshots"}
          </Button>
        </div>
      )}

      {indexState === "ready" && (
        <div>
          {indexedCount < snapshots.length && (
            <div className="flex items-center justify-between gap-3 mb-4 px-4 py-2.5 rounded-lg bg-gray-900/60 border border-gray-800 text-sm">
              <span className="text-gray-400">
                Searching {indexedCount} of {snapshots.length} snapshots
                {indexAllInProgress && " · indexing…"}
              </span>
              <Button variant="ghost" size="sm" onClick={handleIndexAll} disabled={indexAllInProgress}>
                {indexAllInProgress ? "Indexing…" : "Index All"}
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
              <svg className="animate-spin w-4 h-4 text-blue-400" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
                <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
              </svg>
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
        {indexAllDoneCount < indexAllTotal ? (
          <div className="py-2">
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
              You can close this and keep browsing — indexing continues in the background.
            </p>
            <div className="flex justify-end">
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
