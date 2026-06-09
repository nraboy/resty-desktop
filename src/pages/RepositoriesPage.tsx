import React, { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { addRepo, checkRepo, initRepo, listRepos, removeRepo, renameRepo } from "../lib/invoke";
import type { Repository } from "../lib/types";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

type ModalMode = "add" | "init" | null;

export default function RepositoriesPage() {
  const navigate = useNavigate();
  const [repos, setRepos] = useState<Repository[]>([]);
  const [modalMode, setModalMode] = useState<ModalMode>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [form, setForm] = useState({ name: "", path: "", password: "" });
  const [pathMode, setPathMode] = useState<"local" | "remote">("local");
  const [editTarget, setEditTarget] = useState<Repository | null>(null);
  const [editName, setEditName] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<Repository | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; message: string } | null>(null);

  const load = () => listRepos().then(setRepos).catch(() => {});

  useEffect(() => {
    load();
  }, []);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.name || !form.path || !form.password) {
      setError("All fields are required.");
      return;
    }
    setLoading(true);
    setError("");
    const repo: Repository = { id: crypto.randomUUID(), ...form };
    try {
      if (modalMode === "init") {
        await initRepo(repo);
      }
      await addRepo(repo);
      await load();
      setModalMode(null);
      setForm({ name: "", path: "", password: "" });
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

  const handleRename = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!editTarget || !editName.trim()) return;
    setRenaming(true);
    try {
      await renameRepo(editTarget.id, editName.trim());
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
    const tempRepo = { id: "test", name: "", path: form.path, password: form.password };
    try {
      await checkRepo(tempRepo);
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

  return (
    <div className="p-6">
      <div className="flex items-center justify-between mb-6">
        <div>
          <h1 className="text-xl font-semibold text-gray-100">Repositories</h1>
          <p className="text-sm text-gray-500 mt-0.5">Manage your Restic backup repositories</p>
        </div>
        <div className="flex gap-2">
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
          action={
            <Button onClick={() => openModal("init")}>+ New Repository</Button>
          }
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
            >
              <div className="flex items-center gap-3">
                <div>
                  <p className="text-sm font-medium text-gray-100">{repo.name}</p>
                  <p className="text-xs text-gray-500 mt-0.5 font-mono">{repo.path}</p>
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    setEditTarget(repo);
                    setEditName(repo.name);
                  }}
                  className="text-gray-500 hover:text-blue-400"
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
                    setDeleteTarget(repo);
                  }}
                  className="text-gray-500 hover:text-red-400"
                >
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16" />
                  </svg>
                </Button>
              </div>
            </div>
          ))}
        </div>
      )}

      <Modal
        title="Remove Repository"
        open={!!deleteTarget}
        onClose={() => !deleting && setDeleteTarget(null)}
      >
        <p className="text-sm text-gray-300 mb-5">
          Are you sure you want to remove{" "}
          <span className="font-semibold text-white">{deleteTarget?.name}</span>?
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
        title="Rename Repository"
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
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="secondary" type="button" onClick={() => setEditTarget(null)}>
              Cancel
            </Button>
            <Button type="submit" loading={renaming}>
              Save
            </Button>
          </div>
        </form>
      </Modal>

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
          {error && <p className="text-sm text-red-400">{error}</p>}
          <div className="flex items-center justify-between pt-2">
            <Button type="button" variant="secondary" loading={testing} onClick={handleTest}>
              Test Connection
            </Button>
            <div className="flex gap-2">
              <Button variant="secondary" type="button" onClick={() => setModalMode(null)}>
                Cancel
              </Button>
              <Button type="submit" loading={loading}>
                {modalMode === "init" ? "Create & Init" : "Open"}
              </Button>
            </div>
          </div>
        </form>
      </Modal>
    </div>
  );
}
