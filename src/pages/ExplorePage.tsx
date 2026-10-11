import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { listRepoChildren, listRepos, listSnapshots } from "../lib/invoke";
import type { RepoFileHit, Repository, Snapshot } from "../lib/types";
import { formatDateOnly, formatSize } from "../lib/format";
import { toSegments } from "../lib/difftree";
import Button from "../components/Button";
import ActionButton from "../components/ActionButton";
import EmptyState from "../components/EmptyState";
import Spinner from "../components/Spinner";
import { ChevronRightIcon, FileListIcon } from "../components/icons";

// Deliberate copy of BrowsePage's/RepoSearchPage's FileIcon, matching the duplication this
// project already records as known and deferred (see CLAUDE.md, "Settled decisions").
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

// Build the path stack for BrowsePage navigation so pressing Back works naturally. A copy of
// RepoSearchPage's helper of the same name — same reason as FileIcon above.
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

export default function ExplorePage() {
  const { repoId } = useParams<{ repoId: string }>();
  const navigate = useNavigate();

  // The current folder lives in the address, not in component state, so the back button walks
  // back up the tree and a deep folder's URL can be pasted or bookmarked — consistent with
  // this app's URL-based navigation posture (no global store).
  const [searchParams, setSearchParams] = useSearchParams();
  const currentPath = searchParams.get("path") ?? "";

  const [repo, setRepo] = useState<Repository | null>(null);
  const [snapshots, setSnapshots] = useState<Snapshot[]>([]);
  const [entries, setEntries] = useState<RepoFileHit[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showHidden, setShowHidden] = useState(false);

  useEffect(() => {
    if (!repoId) return;
    listRepos()
      .then((repos) => setRepo(repos.find((r) => r.id === repoId) ?? null))
      .catch(() => {});
  }, [repoId]);

  // Only for naming each row's source snapshot by date, and for handing BrowsePage the
  // snapshot it would otherwise have to backfill. A pure-DB read; its failure must not stop
  // the listing, which carries the short id on every row regardless.
  useEffect(() => {
    if (!repoId) return;
    let cancelled = false;
    listSnapshots(repoId)
      .then((snaps) => { if (!cancelled) setSnapshots(snaps); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [repoId]);

  const snapshotById = useMemo(() => {
    const map = new Map<string, Snapshot>();
    for (const s of snapshots) map.set(s.id, s);
    return map;
  }, [snapshots]);

  // Same out-of-order guard as BrowsePage's loadSeqRef: walking the tree quickly (or
  // back/forward through history) can resolve out of order, and a stale response would render
  // one folder's rows under another's breadcrumb.
  const loadSeqRef = useRef(0);
  const load = useCallback(async (path: string) => {
    if (!repoId) return;
    const seq = ++loadSeqRef.current;
    setLoading(true);
    setError("");
    try {
      const data = await listRepoChildren(repoId, path);
      if (seq !== loadSeqRef.current) return; // a newer navigation superseded this one
      setEntries(data);
    } catch (err: any) {
      if (seq !== loadSeqRef.current) return;
      setError(String(err));
      setEntries([]); // stale rows no longer match the breadcrumb
    } finally {
      if (seq === loadSeqRef.current) setLoading(false);
    }
  }, [repoId]);

  useEffect(() => { load(currentPath); }, [load, currentPath]);

  // Reuses the diff view's segmentation helper rather than introducing another. Each crumb's
  // path is rebuilt with its leading separator, which is the form `parent_path` stores.
  const trail = useMemo(
    () => toSegments(currentPath).map((name, i, segs) => ({
      name,
      path: "/" + segs.slice(0, i + 1).join("/"),
    })),
    [currentPath]
  );

  const openFolder = useCallback((path: string) => {
    setSearchParams(path ? { path } : {});
  }, [setSearchParams]);

  const visibleEntries = useMemo(() =>
    entries
      .filter((entry) => showHidden || !entry.name.startsWith("."))
      .sort((a, b) => {
        if (a.type === "dir" && b.type !== "dir") return -1;
        if (a.type !== "dir" && b.type === "dir") return 1;
        return a.name.localeCompare(b.name);
      }),
  [entries, showHidden]);

  const hiddenCount = entries.length - visibleEntries.length;

  const showInSnapshot = (entry: RepoFileHit) => {
    const { path, stack } = browseTarget(entry);
    navigate(`/snapshots/${repoId}/${entry.snapshotId}/browse`, {
      state: {
        snapshot: snapshotById.get(entry.snapshotId),
        initialPath: path,
        initialPathStack: stack,
        fromList: true,
        backLabel: "Explore",
      },
    });
  };

  if (!repoId) {
    return (
      <EmptyState
        title="Repository not found"
        action={<Button variant="secondary" onClick={() => navigate("/")}>Go to Repositories</Button>}
      />
    );
  }

  return (
    <div className="p-6">
      <div className="mb-6">
        <div className="flex items-center gap-3">
          <Button variant="ghost" size="sm" onClick={() => navigate("/")}>
            ← Repositories
          </Button>
        </div>
        <h1 className="text-xl font-semibold text-gray-100 mt-3">Explore Repository</h1>
        <p className="text-sm text-gray-500 mt-0.5">
          Every folder this repository has ever held, as one tree. Each path appears once, at its
          newest backed-up version, so a file deleted months ago is still here.
        </p>
        {repo && <p className="text-sm text-gray-400 mt-1">{repo.name}</p>}
      </div>

      <div className="flex items-center gap-1 mb-4 text-sm text-gray-400 justify-between">
        {trail.length === 0 ? (
          <span className="text-gray-300">root</span>
        ) : (
          <button onClick={() => openFolder("")} className="hover:text-gray-200 transition-colors">
            root
          </button>
        )}
        {trail.map((crumb, i) => (
          <span key={crumb.path} className="contents">
            <span className="text-gray-500">/</span>
            {i === trail.length - 1 ? (
              <span className="text-gray-300">{crumb.name}</span>
            ) : (
              <button
                onClick={() => openFolder(crumb.path)}
                className="hover:text-gray-200 transition-colors"
              >
                {crumb.name}
              </button>
            )}
          </span>
        ))}
        <div className="ml-auto pl-4 flex items-center gap-4">
          <label className="flex items-center gap-2 select-none">
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.target.checked)}
              className="w-4 h-4 accent-blue-500"
            />
            Show hidden files
          </label>
        </div>
      </div>

      {error && (
        <div className="mb-4 p-3 bg-red-900/30 border border-red-700 rounded-lg text-sm text-red-300">
          {error}
        </div>
      )}

      <div className="rounded-xl border border-gray-800 overflow-hidden">
        <table className="w-full text-sm">
          <thead>
            <tr className="bg-gray-900 border-b border-gray-800 text-left">
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider">Name</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-28">Size</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-32">Modified</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-48">From snapshot</th>
              <th className="px-4 py-3 text-xs text-gray-500 font-medium uppercase tracking-wider w-24 wide:w-auto text-right">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-800">
            {loading ? (
              <tr>
                <td colSpan={5}>
                  <div className="flex items-center justify-center py-20 text-gray-500">
                    <Spinner className="w-6 h-6 mr-2 text-current" />
                    Loading…
                  </div>
                </td>
              </tr>
            ) : (
              visibleEntries.map((entry) => {
                const snap = snapshotById.get(entry.snapshotId);
                return (
                  <tr key={entry.path} className="hover:bg-gray-900/50 transition-colors">
                    <td className="px-4 py-2.5">
                      <div className="flex items-center gap-2">
                        <FileIcon type={entry.type} />
                        {entry.type === "dir" ? (
                          <button
                            onClick={() => openFolder(entry.path)}
                            className="text-gray-200 hover:text-gray-50 transition-colors text-left"
                          >
                            {entry.name}
                          </button>
                        ) : (
                          <span className="text-gray-300">{entry.name}</span>
                        )}
                      </div>
                    </td>
                    <td className="px-4 py-2.5 text-gray-500 text-xs">
                      {entry.type === "dir" ? "—" : formatSize(entry.size)}
                    </td>
                    <td className="px-4 py-2.5 text-gray-500 text-xs">
                      {entry.mtime ? formatDateOnly(entry.mtime) : "—"}
                    </td>
                    <td className="px-4 py-2.5 text-xs">
                      <div className="flex items-center gap-2">
                        <span
                          className="font-mono px-1.5 py-0.5 rounded bg-gray-800 text-gray-400 whitespace-nowrap"
                          title="Newest indexed snapshot containing this path"
                        >
                          {entry.snapshotShortId}
                        </span>
                        <span className="text-gray-500 whitespace-nowrap">
                          {snap ? formatDateOnly(snap.time) : "—"}
                        </span>
                      </div>
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <ActionButton
                        label="Snapshot"
                        title="Show in this snapshot's own browse view"
                        tone="blue"
                        onClick={() => showInSnapshot(entry)}
                        icon={<ChevronRightIcon className="w-4 h-4" />}
                      />
                    </td>
                  </tr>
                );
              })
            )}
          </tbody>
        </table>
        {!loading && !error && visibleEntries.length === 0 && (
          <EmptyState
            icon={<FileListIcon className="w-8 h-8" />}
            title={hiddenCount > 0 ? "Only hidden items here" : trail.length === 0 ? "Nothing to explore yet" : "Empty folder"}
            description={
              hiddenCount > 0
                ? `${hiddenCount} hidden item${hiddenCount !== 1 ? "s" : ""} — turn on "Show hidden files" to see them.`
                : trail.length === 0
                  ? "Only fully-indexed snapshots appear in this tree. Index this repository's snapshots to populate it."
                  : "No indexed snapshot holds anything inside this folder."
            }
          />
        )}
      </div>
    </div>
  );
}
