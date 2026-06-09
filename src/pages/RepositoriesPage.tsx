import React, { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { v4 as uuidv4 } from "uuid";
import { open } from "@tauri-apps/plugin-dialog";
import { addRepo, initRepo, listRepos, removeRepo } from "../lib/invoke";
import type { Repository } from "../lib/types";
import { useAppStore } from "../store/appStore";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import EmptyState from "../components/EmptyState";

type ModalMode = "add" | "init" | null;

export default function RepositoriesPage() {
  const navigate = useNavigate();
  const { activeRepo, setActiveRepo } = useAppStore();
  const [repos, setRepos] = useState<Repository[]>([]);
  const [modalMode, setModalMode] = useState<ModalMode>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [form, setForm] = useState({ name: "", path: "", password: "" });

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
    const repo: Repository = { id: uuidv4(), ...form };
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

  const handleRemove = async (id: string) => {
    await removeRepo(id);
    if (activeRepo?.id === id) setActiveRepo(null);
    await load();
  };

  const openModal = (mode: ModalMode) => {
    setForm({ name: "", path: "", password: "" });
    setError("");
    setModalMode(mode);
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
              className={`flex items-center justify-between p-4 rounded-xl border transition-colors cursor-pointer ${
                activeRepo?.id === repo.id
                  ? "bg-blue-600/10 border-blue-600/40"
                  : "bg-gray-900 border-gray-800 hover:border-gray-700"
              }`}
              onClick={() => {
                setActiveRepo(repo);
                navigate("/snapshots");
              }}
            >
              <div className="flex items-center gap-3">
                <div
                  className={`w-3 h-3 rounded-full flex-shrink-0 ${
                    activeRepo?.id === repo.id ? "bg-green-400" : "bg-gray-600"
                  }`}
                />
                <div>
                  <p className="text-sm font-medium text-gray-100">{repo.name}</p>
                  <p className="text-xs text-gray-500 mt-0.5 font-mono">{repo.path}</p>
                </div>
              </div>
              <div className="flex items-center gap-2">
                {activeRepo?.id === repo.id && (
                  <span className="text-xs text-blue-400 px-2 py-0.5 bg-blue-500/20 rounded-full">
                    Active
                  </span>
                )}
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    handleRemove(repo.id);
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
            <label className="block text-sm font-medium text-gray-300 mb-1">
              Repository Path or URL
            </label>
            <div className="flex gap-2">
              <button
                type="button"
                onClick={pickFolder}
                className="flex-1 text-left px-3 py-2 rounded-lg bg-gray-800 border border-gray-700 text-sm font-mono truncate text-gray-400 hover:border-gray-500 transition-colors"
              >
                {form.path || "Click to choose a folder…"}
              </button>
              <Button type="button" variant="secondary" size="sm" onClick={pickFolder}>
                Browse…
              </Button>
            </div>
            <p className="text-xs text-gray-600 mt-1">
              For remote repos (S3, SFTP, etc.) you can type the URL directly.
            </p>
            {form.path && (
              <input
                type="text"
                className="mt-1 w-full px-3 py-1.5 rounded-lg bg-gray-800 border border-gray-700 text-xs font-mono text-gray-300 focus:outline-none focus:border-blue-500"
                value={form.path}
                onChange={(e) => setForm({ ...form, path: e.target.value })}
                placeholder="or edit/paste a remote URL"
              />
            )}
          </div>
          <Input
            label="Password"
            type="password"
            placeholder="Repository password"
            value={form.password}
            onChange={(e) => setForm({ ...form, password: e.target.value })}
          />
          {error && <p className="text-sm text-red-400">{error}</p>}
          <div className="flex justify-end gap-2 pt-2">
            <Button variant="secondary" type="button" onClick={() => setModalMode(null)}>
              Cancel
            </Button>
            <Button type="submit" loading={loading}>
              {modalMode === "init" ? "Create & Init" : "Open"}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
