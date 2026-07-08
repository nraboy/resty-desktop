import { useEffect, useRef, useState, type MouseEvent, type FormEvent } from "react";
import ContextMenu, { type ContextMenuItemDef } from "../components/ContextMenu";
import { useNavigate, useSearchParams } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import {
  addRepo,
  cancelMirror,
  cancelPrune,
  checkRepo,
  getRemoteAutoRefresh,
  getRepoPassword,
  getRepoStats,
  initRepo,
  listRepos,
  mirrorRepo,
  pruneRepo,
  refreshRepoStats,
  refreshSnapshots,
  removeRepo,
  renameRepo,
  updateRepoPassword,
  updateRepoPath,
  testRepoConnection,
} from "../lib/invoke";
import type { CheckResult, Repository, ResticStats } from "../lib/types";
import { isRemoteRepo } from "../lib/types";
import { formatBytes } from "../lib/format";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

type ModalMode = "add" | "init" | null;

export default function RepositoriesPage() {
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [repos, setRepos] = useState<Repository[]>([]);
  const [statsMap, setStatsMap] = useState<Record<string, ResticStats | null>>({});
  const [statsErrorMap, setStatsErrorMap] = useState<Record<string, string>>({});
  const [refreshingRow, setRefreshingRow] = useState<string | null>(null);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [modalMode, setModalMode] = useState<ModalMode>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [form, setForm] = useState({ name: "", path: "", password: "" });
  const [pathMode, setPathMode] = useState<"local" | "remote">("local");
  const [editTarget, setEditTarget] = useState<Repository | null>(null);
  const [editName, setEditName] = useState("");
  const [editPath, setEditPath] = useState("");
  const [editPassword, setEditPassword] = useState("");
  const [editTestResult, setEditTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [editTesting, setEditTesting] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Repository | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);
  const [mirrorSource, setMirrorSource] = useState<Repository | null>(null);
  const [mirrorDestId, setMirrorDestId] = useState("");
  const [mirroring, setMirroring] = useState(false);
  const [mirrorDone, setMirrorDone] = useState(false);
  const [mirrorCancelled, setMirrorCancelled] = useState(false);
  const [mirrorError, setMirrorError] = useState("");
  const [mirrorElapsed, setMirrorElapsed] = useState(0);
  const mirrorStartRef = useRef<number>(0);
  const [contextMenu, setContextMenu] = useState<{ repo: Repository; x: number; y: number } | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkResult, setCheckResult] = useState<CheckResult | null>(null);
  const [checkRepoName, setCheckRepoName] = useState("");
  const [pruneTarget, setPruneTarget] = useState<Repository | null>(null);
  const [pruning, setPruning] = useState(false);
  const [pruneDone, setPruneDone] = useState(false);
  const [pruneCancelled, setPruneCancelled] = useState(false);
  const [pruneError, setPruneError] = useState("");
  const [pruneElapsed, setPruneElapsed] = useState(0);
  const pruneStartRef = useRef<number>(0);

  const [remoteAutoRefresh, setRemoteAutoRefresh] = useState(false);

  const load = () =>
    listRepos()
      .then((r) => { setRepos(r); return r; })
      .catch(() => [] as Repository[]);

  const fetchStatsForLocal = (repoList: Repository[]) => {
    for (const repo of repoList) {
      getRepoStats(repo.id)
        .then((s) => setStatsMap((prev) => ({ ...prev, [repo.id]: s })))
        .catch((err) => {
          setStatsMap((prev) => ({ ...prev, [repo.id]: null }));
          setStatsErrorMap((prev) => ({ ...prev, [repo.id]: String(err) }));
        });
    }
  };

  useEffect(() => {
    getRemoteAutoRefresh().then(setRemoteAutoRefresh).catch(() => {});
    load().then(fetchStatsForLocal);
  }, []);

  useEffect(() => {
    if (searchParams.get("action") === "new-repo") {
      openModal("init");
      setSearchParams({}, { replace: true });
    }
    // setSearchParams is a stable react-router reference
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  const refreshRow = (repo: Repository) => {
    setRefreshingRow(repo.id);
    setStatsMap((prev) => { const next = { ...prev }; delete next[repo.id]; return next; });
    setStatsErrorMap((prev) => { const next = { ...prev }; delete next[repo.id]; return next; });
    refreshRepoStats(repo.id)
      .then((s) => setStatsMap((prev) => ({ ...prev, [repo.id]: s })))
      .catch((err) => {
        setStatsMap((prev) => ({ ...prev, [repo.id]: null }));
        setStatsErrorMap((prev) => ({ ...prev, [repo.id]: String(err) }));
      })
      .finally(() => setRefreshingRow(null));
  };

  const handleRefreshRow = (e: MouseEvent, repo: Repository) => {
    e.stopPropagation();
    refreshRow(repo);
  };

  const handleRefreshAll = async () => {
    setRefreshingAll(true);
    const reposToRefresh = repos.filter((repo) => !isRemoteRepo(repo.path) || remoteAutoRefresh);
    setStatsMap((prev) => {
      const next = { ...prev };
      for (const repo of reposToRefresh) delete next[repo.id];
      return next;
    });
    setStatsErrorMap((prev) => {
      const next = { ...prev };
      for (const repo of reposToRefresh) delete next[repo.id];
      return next;
    });
    await Promise.allSettled(
      reposToRefresh
        .map((repo) =>
          refreshRepoStats(repo.id)
            .then((s) => setStatsMap((prev) => ({ ...prev, [repo.id]: s })))
            .catch((err) => {
              setStatsMap((prev) => ({ ...prev, [repo.id]: null }));
              setStatsErrorMap((prev) => ({ ...prev, [repo.id]: String(err) }));
            })
        )
    );
    setRefreshingAll(false);
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!form.name || !form.path || !form.password) {
      setError("All fields are required.");
      return;
    }
    setLoading(true);
    setError("");
    const id = crypto.randomUUID();
    try {
      if (modalMode === "init") {
        await initRepo(id, form.name, form.path, form.password);
      } else {
        await addRepo(id, form.name, form.path, form.password);
      }
      await load();
      setModalMode(null);
      setForm({ name: "", path: "", password: "" });
      if (!isRemoteRepo(form.path)) {
        refreshSnapshots(id).catch(() => {});
        refreshRepoStats(id)
          .then((s) => setStatsMap((prev) => ({ ...prev, [id]: s })))
          .catch(() => {});
      }
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleRemove = async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      await removeRepo(deleteTarget.id);
      setDeleteTarget(null);
      await load();
    } finally {
      setDeleting(false);
    }
  };

  const handleRename = async (e: FormEvent) => {
    e.preventDefault();
    if (!editTarget || !editName.trim() || !editPath.trim() || !editPassword.trim()) return;
    setRenaming(true);
    try {
      if (editName.trim() !== editTarget.name) {
        await renameRepo(editTarget.id, editName.trim());
      }
      if (editPath.trim() !== editTarget.path) {
        await updateRepoPath(editTarget.id, editPath.trim());
      }
      const originalPassword = await getRepoPassword(editTarget.id);
      if (editPassword.trim() !== originalPassword) {
        await updateRepoPassword(editTarget.id, editPassword.trim());
      }
      await load();
      setEditTarget(null);
    } finally {
      setRenaming(false);
    }
  };

  const openModal = (mode: ModalMode) => {
    setForm({ name: "", path: "", password: "" });
    setError("");
    setTestResult(null);
    setPathMode("local");
    setModalMode(mode);
  };

  const handleTest = async () => {
    if (!form.path || !form.password) {
      setTestResult({ ok: false, message: "Path and password are required to test." });
      return;
    }
    setTesting(true);
    setTestResult(null);
    try {
      await testRepoConnection(form.path, form.password);
      setTestResult({ ok: true, message: "Connection successful — repository is accessible." });
    } catch (err: any) {
      setTestResult({ ok: false, message: String(err) });
    } finally {
      setTesting(false);
    }
  };

  const pickFolder = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected) setForm((f) => ({ ...f, path: selected as string }));
  };

  useEffect(() => {
    if (!mirroring) return;
    mirrorStartRef.current = Date.now();
    setMirrorElapsed(0);
    const id = setInterval(() => {
      setMirrorElapsed(Math.floor((Date.now() - mirrorStartRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [mirroring]);

  useEffect(() => {
    if (!pruning) return;
    pruneStartRef.current = Date.now();
    setPruneElapsed(0);
    const id = setInterval(() => {
      setPruneElapsed(Math.floor((Date.now() - pruneStartRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [pruning]);

  const handleMirror = async () => {
    if (!mirrorSource || !mirrorDestId) return;
    setMirroring(true);
    setMirrorDone(false);
    setMirrorCancelled(false);
    setMirrorError("");
    try {
      await mirrorRepo(mirrorSource.id, mirrorDestId);
      setMirrorDone(true);
      setStatsMap((prev) => { const next = { ...prev }; delete next[mirrorDestId]; return next; });
      refreshRepoStats(mirrorDestId)
        .then((s) => setStatsMap((prev) => ({ ...prev, [mirrorDestId]: s })))
        .catch(() => {});
    } catch (err: any) {
      const msg = String(err);
      if (msg === "cancelled") {
        setMirrorCancelled(true);
      } else {
        setMirrorError(msg);
      }
    } finally {
      setMirroring(false);
    }
  };

  const handleCancelMirror = async () => {
    try { await cancelMirror(); } catch {}
  };

  const closeMirrorModal = () => {
    if (mirroring) return;
    setMirrorSource(null);
    setMirrorDestId("");
    setMirrorDone(false);
    setMirrorCancelled(false);
    setMirrorError("");
  };

  const handlePrune = async () => {
    if (!pruneTarget) return;
    setPruning(true);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    try {
      await pruneRepo(pruneTarget.id);
      setPruneDone(true);
    } catch (err: any) {
      const msg = String(err);
      if (msg === "Cancelled") {
        setPruneCancelled(true);
      } else {
        setPruneError(msg);
      }
    } finally {
      setPruning(false);
    }
  };

  const handleCancelPrune = async () => {
    try { await cancelPrune(); } catch {}
  };

  const closePruneModal = () => {
    if (pruning) return;
    setPruneTarget(null);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
  };

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Repositories</h1>
          <p className="text-sm text-gray-500 mt-0.5">Manage your Restic backup repositories</p>
        </div>
        <div className="flex gap-2">
          {repos.length > 0 && (
            <Button
              variant="secondary"
              disabled={refreshingAll}
              onClick={handleRefreshAll}
            >
              Refresh Stats
            </Button>
          )}
          <Button variant="secondary" onClick={() => openModal("add")}>
            Open Existing
          </Button>
          <Button onClick={() => openModal("init")}>
            + New Repository
          </Button>
        </div>
      </div>

      {repos.length === 0 ? (
        <EmptyState
          title="No repositories yet"
          description="Create a new repository or open an existing one."
          action={<Button onClick={() => openModal("init")}>+ New Repository</Button>}
          icon={
            <svg className="w-12 h-12" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1}
                d="M3 7a2 2 0 012-2h14a2 2 0 012 2v10a2 2 0 01-2 2H5a2 2 0 01-2-2V7z" />
            </svg>
          }
        />
      ) : (
        <div className="grid gap-3">
          {repos.map((repo) => (
            <div
              key={repo.id}
              className="flex items-center justify-between p-4 rounded-xl border bg-gray-900 border-gray-800 hover:border-gray-700 transition-colors cursor-pointer"
              onClick={() => navigate(`/snapshots/${repo.id}`)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setContextMenu({ repo, x: e.clientX, y: e.clientY });
              }}
            >
              <div className="flex items-center gap-3">
                <div>
                  <p className="text-sm font-medium text-gray-100">{repo.name}</p>
                  <p className="text-xs text-gray-500 mt-0.5 font-mono">{repo.path}</p>
                </div>
              </div>
              <div className="flex items-center gap-4">
                <div className="flex items-center gap-2">
                  <div className="text-right min-w-[80px]">
                    {repo.id in statsMap ? (
                      statsMap[repo.id] ? (
                        <>
                          <p className="text-sm font-medium text-gray-300">{formatBytes(statsMap[repo.id]!.total_size)}</p>
                          <p className="text-xs text-gray-600">{statsMap[repo.id]!.snapshots_count} snapshot{statsMap[repo.id]!.snapshots_count !== 1 ? "s" : ""}</p>
                        </>
                      ) : (
                        <div className="relative group cursor-help">
                          <p className="text-xs text-gray-600">unavailable</p>
                          {statsErrorMap[repo.id] && (
                            <div className="absolute bottom-full right-0 mb-1 px-2 py-1.5 w-72 bg-gray-900 border border-gray-700 text-gray-300 text-xs rounded shadow-lg opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-50 break-words">
                              {statsErrorMap[repo.id]}
                            </div>
                          )}
                        </div>
                      )
                    ) : isRemoteRepo(repo.path) ? (
                      <p className="text-xs text-gray-600">—</p>
                    ) : (
                      <p className="text-xs text-gray-600 animate-pulse">loading…</p>
                    )}
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={refreshingAll || refreshingRow === repo.id}
                    onClick={(e) => handleRefreshRow(e, repo)}
                    className="text-gray-500 hover:text-blue-400"
                    title="Refresh stats"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
                    </svg>
                  </Button>
                </div>
                <div className="flex items-center gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setEditTarget(repo);
                      setEditName(repo.name);
                      setEditPath(repo.path);
                      setEditPassword("");
                      setEditTestResult(null);
                      getRepoPassword(repo.id).then(setEditPassword).catch(() => {});
                    }}
                    className="text-gray-500 hover:text-blue-400"
                    title="Rename"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M11 5H6a2 2 0 00-2 2v11a2 2 0 002 2h11a2 2 0 002-2v-5m-1.414-9.414a2 2 0 112.828 2.828L11.828 15H9v-2.828l8.586-8.586z" />
                    </svg>
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setMirrorSource(repo);
                      setMirrorDestId("");
                      setMirrorDone(false);
                      setMirrorCancelled(false);
                      setMirrorError("");
                    }}
                    className="text-gray-500 hover:text-purple-400"
                    title="Mirror to another repository"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M8 7h12m0 0l-4-4m4 4l-4 4m0 6H4m0 0l4 4m-4-4l4-4" />
                    </svg>
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDeleteTarget(repo);
                    }}
                    className="text-gray-500 hover:text-red-300"
                    title="Remove"
                  >
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                        d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                    </svg>
                  </Button>
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        title={`Check: ${checkRepoName}`}
        open={checking || checkResult !== null}
        onClose={() => { if (!checking) setCheckResult(null); }}
      >
        {checking ? (
          <div className="flex flex-col items-center justify-center py-10 gap-3 text-sm text-gray-400">
            <svg className="animate-spin w-6 h-6 text-blue-400" xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
            </svg>
            Checking repository…
          </div>
        ) : checkResult && (
          <>
            {checkResult.errors.length === 0 ? (
              <div className={`flex flex-col items-center justify-center py-8 gap-2 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-300"}`}>
                {checkResult.success ? (
                  <>
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-8 h-8 shrink-0">
                      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
                    </svg>
                    No errors found
                  </>
                ) : (
                  <>
                    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-8 h-8 shrink-0">
                      <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                    </svg>
                    Errors found
                  </>
                )}
              </div>
            ) : (
              <>
                <div className={`flex items-center gap-2 mb-4 text-sm font-medium ${checkResult.success ? "text-green-400" : "text-red-300"}`}>
                  <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                    <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.28 7.22a.75.75 0 00-1.06 1.06L8.94 10l-1.72 1.72a.75.75 0 101.06 1.06L10 11.06l1.72 1.72a.75.75 0 101.06-1.06L11.06 10l1.72-1.72a.75.75 0 00-1.06-1.06L10 8.94 8.28 7.22z" clipRule="evenodd" />
                  </svg>
                  Errors found
                </div>
                <div className="mb-4 space-y-2">
                  {checkResult.errors.map((err, i) => (
                    <div key={i} className="text-xs font-mono bg-red-900/30 border border-red-700 rounded p-2 text-red-300 whitespace-pre-wrap break-all">
                      {err}
                    </div>
                  ))}
                </div>
              </>
            )}
            <div className="flex items-center justify-between mt-4">
              <span className="text-xs text-gray-500">Completed in {checkResult.duration_seconds.toFixed(1)}s</span>
              <Button variant="secondary" onClick={() => setCheckResult(null)}>Close</Button>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title="Remove Repository"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to remove{" "}
          <span className="font-semibold text-gray-50">{deleteTarget?.name}</span>?
          This only removes it from the list — the repository data on disk is not deleted.
        </p>
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={() => setDeleteTarget(null)} disabled={deleting}>
            Cancel
          </Button>
          <Button variant="danger" loading={deleting} onClick={handleRemove}>
            Remove
          </Button>
        </div>
      </Modal>

      <Modal
        title="Edit Repository"
        open={editTarget !== null}
        onClose={() => setEditTarget(null)}
      >
        <form onSubmit={handleRename} className="space-y-4">
          <Input
            label="Name"
            placeholder="e.g. Home Backup"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            autoFocus
          />
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-2">Path</label>
            {editTarget && isRemoteRepo(editTarget.path) ? (
              <input
                type="text"
                className="w-full px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono text-gray-300 focus:outline-none focus:border-blue-500 placeholder-gray-600"
                value={editPath}
                onChange={(e) => setEditPath(e.target.value)}
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
              />
            ) : (
              <div className="flex items-center gap-2">
                <span className={`flex-1 px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono truncate min-w-0 ${editPath ? "text-gray-300" : "text-gray-600"}`}>
                  {editPath || "No folder selected"}
                </span>
                <Button
                  type="button"
                  variant="secondary"
                  size="sm"
                  onClick={async () => {
                    const selected = await open({ directory: true, multiple: false });
                    if (selected) setEditPath(selected as string);
                  }}
                >
                  Browse…
                </Button>
              </div>
            )}
          </div>
          <Input
            label="Password"
            type="password"
            placeholder="Repository password"
            value={editPassword}
            onChange={(e) => { setEditPassword(e.target.value); setEditTestResult(null); }}
          />
          {editTestResult && (
            <div className={`text-sm rounded-lg px-3 py-2 ${editTestResult.ok ? "bg-green-900/40 text-green-300 border border-green-700" : "bg-red-900/40 text-red-300 border border-red-700"}`}>
              {editTestResult.message}
            </div>
          )}
          <div className="flex items-center justify-between pt-2">
            <Button
              type="button"
              variant="secondary"
              loading={editTesting}
              onClick={async () => {
                if (!editPath.trim() || !editPassword.trim()) {
                  setEditTestResult({ ok: false, message: "Path and password are required to test." });
                  return;
                }
                setEditTesting(true);
                setEditTestResult(null);
                try {
                  await testRepoConnection(editPath.trim(), editPassword.trim());
                  setEditTestResult({ ok: true, message: "Connection successful — repository is accessible." });
                } catch (err: any) {
                  setEditTestResult({ ok: false, message: String(err) });
                } finally {
                  setEditTesting(false);
                }
              }}
            >
              Test Connection
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" type="button" onClick={() => setEditTarget(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={renaming}>
                Save
              </Button>
            </div>
          </div>
        </form>
      </Modal>

      <Modal
        title="Mirror Repository"
        open={mirrorSource !== null}
        onClose={closeMirrorModal}
      >
        {mirrorDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Mirror complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              All snapshots from <span className="font-semibold text-gray-50">{mirrorSource?.name}</span> have been copied to{" "}
              <span className="font-semibold text-gray-50">{repos.find((r) => r.id === mirrorDestId)?.name}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeMirrorModal}>Close</Button>
            </div>
          </>
        ) : mirrorCancelled ? (
          <>
            <p className="text-sm text-gray-400 mb-4">Mirror was cancelled.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closeMirrorModal}>Close</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-4">
              Copy all snapshots from <span className="font-semibold text-gray-50">{mirrorSource?.name}</span> into another
              repository. Snapshots that already exist in the destination are skipped.
            </p>
            <div className="mb-4">
              <label className="block text-sm font-medium text-gray-300 mb-2">Destination Repository</label>
              <div className="relative">
                <select
                  className="w-full appearance-none px-3 py-2 pr-8 rounded-lg bg-gray-800 border border-gray-700 text-sm text-gray-300 focus:outline-none focus:border-blue-500"
                  value={mirrorDestId}
                  onChange={(e) => setMirrorDestId(e.target.value)}
                  disabled={mirroring}
                >
                  <option value="">Select a repository…</option>
                  {repos
                    .filter((r) => r.id !== mirrorSource?.id)
                    .map((r) => (
                      <option key={r.id} value={r.id}>{r.name} — {r.path}</option>
                    ))}
                </select>
                <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">▾</div>
              </div>
            </div>
            {mirrorError && (
              <p className="text-sm text-red-300 mb-3">{mirrorError}</p>
            )}
            {mirroring && (
              <div className="mb-4">
                <p className="text-xs text-gray-400 mb-1">Copying snapshots…</p>
                <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
                  <div className="h-2 w-1/3 rounded-full bg-purple-500 animate-[slide_1.4s_ease-in-out_infinite]" />
                </div>
              </div>
            )}
            <div className="flex items-center justify-between">
              {mirroring ? (
                <span className="text-xs text-gray-500">
                  {mirrorElapsed < 60
                    ? `${mirrorElapsed}s elapsed`
                    : `${Math.floor(mirrorElapsed / 60)}m ${mirrorElapsed % 60}s elapsed`}
                </span>
              ) : (
                <span />
              )}
              <div className="flex gap-2">
              {mirroring ? (
                <Button variant="secondary" onClick={handleCancelMirror}>Cancel</Button>
              ) : (
                <>
                  <Button variant="secondary" onClick={closeMirrorModal}>Cancel</Button>
                  <Button
                    onClick={handleMirror}
                    disabled={!mirrorDestId}
                    className="bg-purple-600 hover:bg-purple-500"
                  >
                    Mirror
                  </Button>
                </>
              )}
              </div>
            </div>
          </>
        )}
      </Modal>

      <Modal
        title={`Prune: ${pruneTarget?.name ?? ""}`}
        open={pruneTarget !== null}
        onClose={closePruneModal}
      >
        {pruneDone ? (
          <>
            <div className="flex items-center gap-2 mb-4 text-sm font-medium text-green-400">
              <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20" fill="currentColor" className="w-5 h-5 shrink-0">
                <path fillRule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.857-9.809a.75.75 0 00-1.214-.882l-3.483 4.79-1.88-1.88a.75.75 0 10-1.06 1.061l2.5 2.5a.75.75 0 001.137-.089l4-5.5z" clipRule="evenodd" />
              </svg>
              Prune complete
            </div>
            <p className="text-sm text-gray-400 mb-4">
              Unreferenced data has been removed from{" "}
              <span className="font-semibold text-gray-50">{pruneTarget?.name}</span>.
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </>
        ) : pruneCancelled ? (
          <>
            <p className="text-sm text-gray-400 mb-4">Prune was cancelled.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </>
        ) : pruning ? (
          <>
            <p className="text-xs text-gray-400 mb-1">Pruning repository…</p>
            <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden mb-4">
              <div className="h-2 w-1/3 rounded-full bg-blue-500 animate-[slide_1.4s_ease-in-out_infinite]" />
            </div>
            {pruneError && <p className="text-sm text-red-300 mb-3">{pruneError}</p>}
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </span>
              <Button variant="secondary" onClick={handleCancelPrune}>Cancel</Button>
            </div>
          </>
        ) : (
          <>
            <p className="text-sm text-gray-300 mb-5">
              Remove unreferenced data from{" "}
              <span className="font-semibold text-gray-50">{pruneTarget?.name}</span>? This frees disk space
              by deleting pack files that are no longer referenced by any snapshot.
            </p>
            {pruneError && <p className="text-sm text-red-300 mb-3">{pruneError}</p>}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closePruneModal}>Cancel</Button>
              <Button onClick={handlePrune}>Prune</Button>
            </div>
          </>
        )}
      </Modal>

      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Open Snapshots",
              onClick: () => navigate(`/snapshots/${contextMenu.repo.id}`),
            },
            {
              label: "Search Files…",
              onClick: () => navigate(`/snapshots/${contextMenu.repo.id}/search`),
            },
            { separator: true },
            {
              label: "Refresh Stats",
              disabled: refreshingAll || refreshingRow === contextMenu.repo.id,
              onClick: () => refreshRow(contextMenu.repo),
            },
            {
              label: "Check Repository",
              onClick: () => {
                const repo = contextMenu.repo;
                setCheckRepoName(repo.name);
                setCheckResult(null);
                setChecking(true);
                checkRepo(repo.id)
                  .then(setCheckResult)
                  .catch((err) => setCheckResult({ success: false, errors: [String(err)], duration_seconds: 0 }))
                  .finally(() => setChecking(false));
              },
            },
            {
              label: "Edit",
              onClick: () => {
                const repo = contextMenu.repo;
                setEditTarget(repo);
                setEditName(repo.name);
                setEditPath(repo.path);
                setEditPassword("");
                setEditTestResult(null);
                getRepoPassword(repo.id).then(setEditPassword).catch(() => {});
              },
            },
            {
              label: "Mirror…",
              onClick: () => {
                setMirrorSource(contextMenu.repo);
                setMirrorDestId("");
                setMirrorDone(false);
                setMirrorCancelled(false);
                setMirrorError("");
              },
            },
            {
              label: "Prune…",
              onClick: () => {
                setPruneTarget(contextMenu.repo);
                setPruneDone(false);
                setPruneCancelled(false);
                setPruneError("");
              },
            },
            { separator: true },
            {
              label: "Delete",
              variant: "danger",
              onClick: () => setDeleteTarget(contextMenu.repo),
            },
          ] satisfies ContextMenuItemDef[]}
        />
      )}

      <Modal
        title={modalMode === "init" ? "Create New Repository" : "Open Existing Repository"}
        open={modalMode !== null}
        onClose={() => setModalMode(null)}
      >
        <form onSubmit={handleSubmit} className="space-y-4">
          <Input
            label="Name"
            placeholder="e.g. Home Backup"
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
          />
          <div>
            <label className="block text-sm font-medium text-gray-300 mb-2">
              Repository Location
            </label>
            <div className="flex rounded-lg overflow-hidden border border-gray-700 mb-3">
              <button
                type="button"
                onClick={() => { setPathMode("local"); setForm((f) => ({ ...f, path: "" })); setTestResult(null); }}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${pathMode === "local" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Local Path
              </button>
              <button
                type="button"
                onClick={() => { setPathMode("remote"); setForm((f) => ({ ...f, path: "" })); setTestResult(null); }}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${pathMode === "remote" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Remote URL
              </button>
            </div>
            {pathMode === "local" ? (
              <div className="flex items-center gap-2">
                <span className={`flex-1 px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono truncate min-w-0 ${form.path ? "text-gray-300" : "text-gray-600"}`}>
                  {form.path || "No folder selected"}
                </span>
                <Button type="button" variant="secondary" size="sm" onClick={pickFolder}>
                  Browse…
                </Button>
              </div>
            ) : (
              <input
                type="text"
                className="w-full px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono text-gray-300 focus:outline-none focus:border-blue-500 placeholder-gray-600"
                value={form.path}
                onChange={(e) => { setForm({ ...form, path: e.target.value }); setTestResult(null); }}
                placeholder="s3:s3.amazonaws.com/bucket or sftp:user@host:/path"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                autoComplete="off"
                autoFocus
              />
            )}
          </div>
          <Input
            label="Password"
            type="password"
            placeholder="Repository password"
            value={form.password}
            onChange={(e) => { setForm({ ...form, password: e.target.value }); setTestResult(null); }}
          />
          {testResult && (
            <div className={`text-sm rounded-lg px-3 py-2 ${testResult.ok ? "bg-green-900/40 text-green-300 border border-green-700" : "bg-red-900/40 text-red-300 border border-red-700"}`}>
              {testResult.message}
            </div>
          )}
          {error && <p className="text-sm text-red-300">{error}</p>}
          <div className="flex items-center justify-between pt-2">
            {modalMode === "add" && (
              <Button type="button" variant="secondary" loading={testing} onClick={handleTest}>
                Test Connection
              </Button>
            )}
            <div className="flex gap-2 ml-auto">
              <Button variant="secondary" type="button" onClick={() => setModalMode(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={loading}>
                {modalMode === "init" ? "Create" : "Open"}
              </Button>
            </div>
          </div>
        </form>
      </Modal>
    </div>
  );
}
