import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { getSnapshotIndexStatus, indexSnapshot, searchSnapshotFiles } from "../lib/invoke";
import type { FileEntry, Snapshot } from "../lib/types";
import { formatDate, formatSize } from "../lib/format";
import Button from "../components/Button";
import Input from "../components/Input";
import Spinner from "../components/Spinner";

type IndexState = "loading" | "not_indexed" | "indexing" | "ready";

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
function browseTarget(entry: FileEntry): { path: string | undefined; stack: string[] } {
  const parts = entry.path.split("/").filter(Boolean);
  const targetParts = entry.type === "dir" ? parts : parts.slice(0, -1);
  const path = targetParts.length ? "/" + targetParts.join("/") : undefined;
  const stack: string[] = [""];
  for (let i = 0; i < targetParts.length - 1; i++) {
    stack.push("/" + targetParts.slice(0, i + 1).join("/"));
  }
  return { path, stack };
}

export default function SearchPage() {
  const { repoId, snapshotId } = useParams<{ repoId: string; snapshotId: string }>();
  const navigate = useNavigate();
  const location = useLocation();
  const locationState = location.state as { snapshot?: Snapshot; fromBrowse?: boolean; returnPath?: string; returnStack?: string[]; restoredQuery?: string; restoredResults?: FileEntry[] } | null;
  const snapshot = locationState?.snapshot;
  const fromBrowse = locationState?.fromBrowse ?? false;
  const returnPath = locationState?.returnPath;
  const returnStack = locationState?.returnStack;

  const [indexState, setIndexState] = useState<IndexState>("loading");
  const [indexError, setIndexError] = useState("");
  const [query, setQuery] = useState(locationState?.restoredQuery ?? "");
  const [results, setResults] = useState<FileEntry[]>(locationState?.restoredResults ?? []);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [searched, setSearched] = useState(locationState?.restoredQuery != null && locationState.restoredQuery.trim().length > 0);

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    return () => { if (debounceRef.current) clearTimeout(debounceRef.current); };
  }, []);

  useEffect(() => {
    if (!repoId || !snapshotId) return;
    getSnapshotIndexStatus(repoId)
      .then((statusMap) => {
        const s = statusMap[snapshotId];
        if (s === "complete") {
          setIndexState("ready");
          setTimeout(() => inputRef.current?.focus(), 50);
        } else if (s === "in_progress") {
          setIndexState("indexing");
        } else {
          setIndexState("not_indexed");
        }
      })
      .catch(() => setIndexState("not_indexed"));
  }, [repoId, snapshotId]);

  useEffect(() => {
    if (!repoId || !snapshotId) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    listen<{ snapshotId: string; success: boolean }>("index:done", (e) => {
      if (e.payload.snapshotId !== snapshotId) return;
      if (e.payload.success) {
        setIndexState("ready");
        setTimeout(() => inputRef.current?.focus(), 50);
      } else {
        setIndexError("Indexing failed. Try again.");
        setIndexState("not_indexed");
      }
    }).then((u) => {
      if (cancelled) { u(); return; }
      unlisten = u;
      // Re-check after the listener is active: if index:done fired in the gap between
      // the initial status check and listen() resolving, we'd be permanently stuck on
      // "indexing". This catches that race.
      getSnapshotIndexStatus(repoId).then((statusMap) => {
        if (!cancelled && statusMap[snapshotId] === "complete") {
          setIndexState("ready");
          setTimeout(() => inputRef.current?.focus(), 50);
        }
      }).catch(() => {});
    });
    return () => { cancelled = true; unlisten?.(); };
  }, [repoId, snapshotId]);

  const handleIndexNow = async () => {
    if (!repoId || !snapshotId) return;
    setIndexError("");
    setIndexState("indexing");
    try {
      const started = await indexSnapshot(repoId, snapshotId);
      if (!started) {
        // Warmer completed between mount and click — re-check rather than wait
        // for an index:done that will never arrive.
        const statusMap = await getSnapshotIndexStatus(repoId);
        if (statusMap[snapshotId] === "complete") setIndexState("ready");
        // If in_progress, the index:done listener above will transition us.
      }
    } catch {
      setIndexError("Failed to start indexing.");
      setIndexState("not_indexed");
    }
  };

  // Guards against out-of-order responses: the backend query can take a second
  // or more, so a burst of keystrokes can have several searches in flight at
  // once. Only the response matching the latest call is allowed to update state.
  const searchSeqRef = useRef(0);

  const runSearch = useCallback(async (q: string) => {
    const seq = ++searchSeqRef.current;
    if (!repoId || !snapshotId || !q.trim()) {
      setResults([]);
      setSearched(false);
      return;
    }
    setSearching(true);
    setSearchError("");
    try {
      const data = await searchSnapshotFiles(repoId, snapshotId, q);
      if (seq !== searchSeqRef.current) return;
      setResults(data);
      setSearched(true);
    } catch (err: any) {
      if (seq !== searchSeqRef.current) return;
      setSearchError(String(err));
    } finally {
      if (seq === searchSeqRef.current) setSearching(false);
    }
  }, [repoId, snapshotId]);

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

  const handleResultClick = (entry: FileEntry) => {
    const { path, stack } = browseTarget(entry);
    // Persist query+results into the current history entry so navigate(-1) from BrowsePage restores them.
    window.history.replaceState(
      { ...window.history.state, usr: { ...locationState, restoredQuery: query, restoredResults: results } },
      ''
    );
    navigate(`/snapshots/${repoId}/${snapshotId}/browse`, {
      state: { snapshot, initialPath: path, initialPathStack: stack, fromSearch: true },
    });
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={() => fromBrowse
            ? navigate(`/snapshots/${repoId}/${snapshotId}/browse`, { state: { snapshot, initialPath: returnPath, initialPathStack: returnStack ?? [] } })
            : navigate(`/snapshots/${repoId}`)}>
            {fromBrowse ? "← Browser" : "← Snapshots"}
          </Button>
        </div>
        <h1 className="text-xl font-semibold text-gray-100 mt-3">Search Files</h1>
        {snapshot && (
          <p className="text-sm text-gray-500 mt-0.5 font-mono">
            {snapshot.short_id} · {formatDate(snapshot.time)}
          </p>
        )}
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
            <p className="text-gray-100 font-medium mb-1">Snapshot not indexed</p>
            <p className="text-sm text-gray-500">
              File search requires an index. Indexing reads the snapshot's file list once and stores it locally — it may take a moment for large snapshots.
            </p>
          </div>
          {indexError && (
            <p className="text-sm text-red-300">{indexError}</p>
          )}
          <Button onClick={handleIndexNow}>
            Index Now
          </Button>
        </div>
      )}

      {indexState === "indexing" && (
        <div className="flex flex-col items-center justify-center py-16 gap-5 text-center max-w-sm mx-auto">
          <div className="p-4 rounded-full bg-gray-800">
            <Spinner className="w-10 h-10 text-blue-400" />
          </div>
          <div>
            <p className="text-gray-100 font-medium mb-1">Building file index…</p>
            <p className="text-sm text-gray-500">
              This may take a moment depending on how many files are in the snapshot. Search will be available when it's done.
            </p>
          </div>
          <div className="w-full">
            <div className="w-full bg-gray-800 rounded-full h-1.5 overflow-hidden">
              <div className="h-1.5 w-1/3 rounded-full bg-blue-500 animate-[slide_1.4s_ease-in-out_infinite]" />
            </div>
          </div>
          <p className="text-xs text-gray-600">You can navigate away — indexing continues in the background.</p>
        </div>
      )}

      {indexState === "ready" && (
        <div>
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
              <p className="text-sm">Type to search across all files in this snapshot</p>
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
    </div>
  );
}
