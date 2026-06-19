import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { activateTray, cancelPrune, changeMasterPassword, clearBrowseCache, deactivateTray, getCompression, getRemoteAutoRefresh, getResticPath, getResticVersion, getRestorePath, getTrayEnabled, pruneAllRepos, setCompression as saveCompression, setRemoteAutoRefresh, setResticPath, setRestorePath, setTrayEnabled } from "../lib/invoke";
import { useTheme } from "../lib/theme";
import type { Theme } from "../lib/theme";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";

const THEMES: { value: Theme; label: string; description: string }[] = [
  { value: "system", label: "System", description: "Follow the OS appearance" },
  { value: "light",  label: "Light",  description: "Always use the light theme" },
  { value: "dark",   label: "Dark",   description: "Always use the dark theme" },
];

export default function SettingsPage() {
  const { theme, setTheme } = useTheme();
  const [resticPath, setResticPathLocal] = useState("restic");
  const [compression, setCompression] = useState("auto");
  const [restorePath, setRestorePathLocal] = useState("");
  const [resticVersion, setResticVersion] = useState<string | null>(null);
  const [versionError, setVersionError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  const [clearingCache, setClearingCache] = useState(false);
  const [cacheCleared, setCacheCleared] = useState(false);

  const [pruneModalOpen, setPruneModalOpen] = useState(false);
  const [pruning, setPruning] = useState(false);
  const [pruneDone, setPruneDone] = useState(false);
  const [pruneCancelled, setPruneCancelled] = useState(false);
  const [pruneError, setPruneError] = useState("");
  const [pruneCurrent, setPruneCurrent] = useState(0);
  const [pruneTotal, setPruneTotal] = useState(0);
  const [pruneRepoName, setPruneRepoName] = useState("");
  const [pruneElapsed, setPruneElapsed] = useState(0);
  const pruneStartRef = useRef<number>(0);
  const pruneUnlistenRef = useRef<(() => void) | null>(null);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cacheTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const passwordTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [trayEnabled, setTrayEnabledLocal] = useState(true);
  const [remoteAutoRefresh, setRemoteAutoRefreshLocal] = useState(false);

  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [changingPassword, setChangingPassword] = useState(false);
  const [passwordChanged, setPasswordChanged] = useState(false);
  const [passwordError, setPasswordError] = useState("");

  useEffect(() => {
    getResticPath().then(setResticPathLocal).catch(() => {});
    getCompression().then(setCompression).catch(() => {});
    getRestorePath().then(setRestorePathLocal).catch(() => {});
    getTrayEnabled().then(setTrayEnabledLocal).catch(() => {});
    getRemoteAutoRefresh().then(setRemoteAutoRefreshLocal).catch(() => {});
    getResticVersion()
      .then((v) => { setResticVersion(v); setVersionError(""); })
      .catch((e) => { setResticVersion(null); setVersionError(String(e)); });
  }, []);

  useEffect(() => {
    if (!pruning) return;
    pruneStartRef.current = Date.now();
    setPruneElapsed(0);
    const id = setInterval(() => {
      setPruneElapsed(Math.floor((Date.now() - pruneStartRef.current) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [pruning]);

  useEffect(() => {
    return () => {
      pruneUnlistenRef.current?.();
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
      if (cacheTimerRef.current !== null) clearTimeout(cacheTimerRef.current);
      if (passwordTimerRef.current !== null) clearTimeout(passwordTimerRef.current);
    };
  }, []);

  const handleSave = async () => {
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await setResticPath(resticPath);
      await saveCompression(compression);
      await setRestorePath(restorePath);
      setSaved(true);
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
      savedTimerRef.current = setTimeout(() => setSaved(false), 2000);
      getResticVersion()
        .then((v) => { setResticVersion(v); setVersionError(""); })
        .catch((e) => { setResticVersion(null); setVersionError(String(e)); });
    } catch (err: any) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleTrayToggle = async (enabled: boolean) => {
    setTrayEnabledLocal(enabled);
    await setTrayEnabled(enabled).catch(() => {});
    if (enabled) {
      await activateTray().catch(() => {});
    } else {
      await deactivateTray().catch(() => {});
    }
  };

  const handleClearCache = async () => {
    setClearingCache(true);
    try {
      await clearBrowseCache();
      setCacheCleared(true);
      if (cacheTimerRef.current !== null) clearTimeout(cacheTimerRef.current);
      cacheTimerRef.current = setTimeout(() => setCacheCleared(false), 2000);
    } finally {
      setClearingCache(false);
    }
  };

  const handlePruneAll = async () => {
    setPruning(true);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    setPruneCurrent(0);
    setPruneTotal(0);
    setPruneRepoName("");

    const unlisten = await listen<{ current: number; total: number; repoName: string }>(
      "prune:progress",
      ({ payload }) => {
        setPruneCurrent(payload.current);
        setPruneTotal(payload.total);
        setPruneRepoName(payload.repoName);
      }
    );
    pruneUnlistenRef.current = unlisten;

    try {
      await pruneAllRepos();
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
      unlisten();
      pruneUnlistenRef.current = null;
    }
  };

  const closePruneModal = async () => {
    if (pruning) {
      await cancelPrune();
      return;
    }
    pruneUnlistenRef.current?.();
    pruneUnlistenRef.current = null;
    setPruneModalOpen(false);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    setPruneCurrent(0);
    setPruneTotal(0);
    setPruneRepoName("");
  };

  const handleChangePassword = async (e: React.FormEvent) => {
    e.preventDefault();
    setPasswordError("");
    setPasswordChanged(false);
    if (newPassword.length < 8) {
      setPasswordError("New password must be at least 8 characters.");
      return;
    }
    if (newPassword !== confirmPassword) {
      setPasswordError("New passwords do not match.");
      return;
    }
    setChangingPassword(true);
    try {
      await changeMasterPassword(oldPassword, newPassword);
      setPasswordChanged(true);
      setOldPassword("");
      setNewPassword("");
      setConfirmPassword("");
      if (passwordTimerRef.current !== null) clearTimeout(passwordTimerRef.current);
      passwordTimerRef.current = setTimeout(() => setPasswordChanged(false), 3000);
    } catch (err: any) {
      setPasswordError(String(err));
    } finally {
      setChangingPassword(false);
    }
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <h1 className="text-xl font-semibold text-gray-100">Settings</h1>
        <p className="text-sm text-gray-500 mt-0.5">Configure Resty Desktop behavior</p>
      </div>

      <div className="bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Appearance</h2>
        <p className="text-xs text-gray-500 mb-3">Choose the color theme for the application.</p>
        <div className="flex gap-2">
          {THEMES.map(({ value, label, description }) => (
            <button
              key={value}
              onClick={() => setTheme(value)}
              className={[
                "flex-1 rounded-lg border px-3 py-3 text-left transition-colors",
                theme === value
                  ? "border-blue-500 bg-blue-600/20 text-blue-400"
                  : "border-gray-700 bg-gray-800 text-gray-400 hover:border-gray-600 hover:text-gray-300",
              ].join(" ")}
            >
              <p className={`text-sm font-medium ${theme === value ? "text-blue-300" : "text-gray-300"}`}>{label}</p>
              <p className="text-xs mt-0.5">{description}</p>
            </button>
          ))}
        </div>
      </div>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Toggles</h2>
        <div className="space-y-4">
          <div>
            <p className="text-xs text-gray-500 mb-3">
              When enabled, closing the window keeps the app running in the system tray so scheduled
              backups continue to run. Disabling this will quit the app when the window is closed.
            </p>
            <label className="flex items-center gap-3 cursor-pointer select-none">
              <button
                role="switch"
                aria-checked={trayEnabled}
                onClick={() => handleTrayToggle(!trayEnabled)}
                className={[
                  "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900",
                  trayEnabled ? "bg-blue-600" : "bg-gray-700",
                ].join(" ")}
              >
                <span
                  className={[
                    "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                    trayEnabled ? "translate-x-4" : "translate-x-1",
                  ].join(" ")}
                />
              </button>
              <span className="text-sm text-gray-300">Keep app running in tray when window is closed</span>
            </label>
            {!trayEnabled && (
              <p className="mt-3 text-xs text-amber-500">
                Warning: scheduled backups will not run while the app is closed.
              </p>
            )}
          </div>
          <div className="pt-4 border-t border-gray-800">
            <p className="text-xs text-gray-500 mb-3">
              When enabled, snapshot lists and repository stats for remote repositories are
              refreshed automatically on page load, the same as local repositories. Disabled by
              default to avoid unnecessary bandwidth charges from your cloud provider.
            </p>
            <label className="flex items-center gap-3 cursor-pointer select-none">
              <button
                role="switch"
                aria-checked={remoteAutoRefresh}
                onClick={() => {
                  const next = !remoteAutoRefresh;
                  setRemoteAutoRefreshLocal(next);
                  setRemoteAutoRefresh(next).catch(() => {});
                }}
                className={[
                  "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900",
                  remoteAutoRefresh ? "bg-blue-600" : "bg-gray-700",
                ].join(" ")}
              >
                <span
                  className={[
                    "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                    remoteAutoRefresh ? "translate-x-4" : "translate-x-1",
                  ].join(" ")}
                />
              </button>
              <span className="text-sm text-gray-300">Auto-refresh data for remote repositories</span>
            </label>
            {remoteAutoRefresh && (
              <p className="mt-3 text-xs text-amber-500">
                Warning: automatic refresh may incur bandwidth charges with your cloud provider.
              </p>
            )}
          </div>
        </div>
      </div>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5 space-y-5">
        <div>
          <h2 className="text-sm font-medium text-gray-300 mb-1">Restic Binary Path</h2>
          <p className="text-xs text-gray-500 mb-3">
            Path to the <span className="font-mono">restic</span> executable. Defaults to{" "}
            <span className="font-mono text-gray-400">restic</span> (must be on PATH).
          </p>
          <Input
            value={resticPath}
            onChange={(e) => setResticPathLocal(e.target.value)}
            placeholder="restic"
          />
          {resticVersion && (
            <p className="mt-2 text-xs text-green-400 font-mono">{resticVersion}</p>
          )}
          {versionError && (
            <p className="mt-2 text-xs text-red-300">{versionError}</p>
          )}
        </div>
        <div>
          <h2 className="text-sm font-medium text-gray-300 mb-1">Backup Compression</h2>
          <p className="text-xs text-gray-500 mb-3">
            Controls the <span className="font-mono">RESTIC_COMPRESSION</span> level applied to all
            future backups.
          </p>
          <div className="relative">
            <select
              value={compression}
              onChange={(e) => setCompression(e.target.value)}
              className="appearance-none w-full bg-gray-800 border border-gray-700 text-gray-100 text-sm rounded-lg px-3 py-2 pr-8 focus:outline-none focus:ring-1 focus:ring-blue-500"
            >
              <option value="auto">auto — default, balanced compression</option>
              <option value="off">off — no compression, fastest</option>
              <option value="fastest">fastest — minimal compression, low CPU</option>
              <option value="better">better — more compression, more CPU</option>
              <option value="max">max — maximum compression, highest CPU</option>
            </select>
            <svg className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
            </svg>
          </div>
        </div>
        <div>
          <h2 className="text-sm font-medium text-gray-300 mb-1">Default Restore Path</h2>
          <p className="text-xs text-gray-500 mb-3">
            Pre-filled target directory when restoring a snapshot or file. You can still override it
            per restore.
          </p>
          <div className="flex gap-2">
            <div className="flex-1">
              <Input
                value={restorePath}
                onChange={(e) => setRestorePathLocal(e.target.value)}
                placeholder="Select a directory…"
                className="w-full"
              />
            </div>
            <Button
              variant="secondary"
              onClick={async () => {
                const dir = await openDialog({ directory: true, multiple: false });
                if (typeof dir === "string") setRestorePathLocal(dir);
              }}
            >
              Browse
            </Button>
          </div>
        </div>
        {error && <p className="text-sm text-red-300">{error}</p>}
        <div className="flex items-center gap-3">
          <Button onClick={handleSave} loading={saving}>Save Settings</Button>
          {saved && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              Saved
            </span>
          )}
        </div>
      </div>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Master Password</h2>
        <p className="text-xs text-gray-500 mb-4">
          Change the master password used to encrypt your repository credentials.
          All stored passwords are re-encrypted immediately.
        </p>
        <form onSubmit={handleChangePassword} className="space-y-3">
          <Input
            label="Current Password"
            type="password"
            placeholder="Enter current master password"
            value={oldPassword}
            onChange={(e) => setOldPassword(e.target.value)}
          />
          <Input
            label="New Password"
            type="password"
            placeholder="At least 8 characters"
            value={newPassword}
            onChange={(e) => setNewPassword(e.target.value)}
          />
          <Input
            label="Confirm New Password"
            type="password"
            placeholder="Re-enter new password"
            value={confirmPassword}
            onChange={(e) => setConfirmPassword(e.target.value)}
          />
          {passwordError && <p className="text-sm text-red-300">{passwordError}</p>}
          <div className="flex items-center gap-3 pt-1">
            <Button type="submit" loading={changingPassword}>Change Password</Button>
            {passwordChanged && (
              <span className="text-sm text-green-400 flex items-center gap-1">
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
                </svg>
                Password changed
              </span>
            )}
          </div>
        </form>
      </div>

      {!resticVersion && <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Install Restic</h2>
        <p className="text-xs text-gray-500 leading-relaxed">
          Restic must be installed separately. Visit{" "}
          <span className="font-mono text-blue-400">restic.net</span> or install via your package
          manager:
        </p>
        <div className="mt-3 space-y-2">
          {[
            { label: "macOS (Homebrew)", cmd: "brew install restic" },
            { label: "Debian/Ubuntu", cmd: "apt install restic" },
            { label: "Windows (Scoop)", cmd: "scoop install restic" },
          ].map(({ label, cmd }) => (
            <div key={label}>
              <p className="text-xs text-gray-500 mb-1">{label}</p>
              <code className="block text-xs bg-gray-800 text-gray-300 px-3 py-2 rounded-lg font-mono">
                {cmd}
              </code>
            </div>
          ))}
        </div>
      </div>}

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Prune Repositories</h2>
        <p className="text-xs text-gray-500 mb-3">
          Remove orphaned data from all repositories. This cleans up pack files not referenced by
          any snapshot, such as leftovers from interrupted backups or manually forgotten snapshots.
        </p>
        <Button variant="secondary" onClick={() => { setPruneModalOpen(true); handlePruneAll(); }}>
          Prune All Repositories
        </Button>
      </div>

      <Modal open={pruneModalOpen} onClose={closePruneModal} title="Prune All Repositories">
        {pruneDone ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">
              All {pruneTotal} {pruneTotal === 1 ? "repository has" : "repositories have"} been pruned successfully.
            </p>
            <div className="flex items-center justify-between">
              <p className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </p>
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </div>
        ) : pruneCancelled ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">Prune was cancelled.</p>
            <div className="flex items-center justify-between">
              <p className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </p>
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </div>
        ) : pruneError ? (
          <div className="space-y-4">
            <p className="text-sm text-red-300">{pruneError}</p>
            <div className="flex items-center justify-between">
              <p className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </p>
              <Button variant="secondary" onClick={closePruneModal}>Close</Button>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            {pruneTotal > 0 ? (
              <p className="text-sm text-gray-400">
                Pruning <span className="text-gray-50 font-medium">{pruneRepoName}</span>
                {" "}({pruneCurrent + 1} of {pruneTotal})…
              </p>
            ) : (
              <p className="text-sm text-gray-400">Starting…</p>
            )}
            <div className="w-full bg-gray-800 rounded-full h-2 overflow-hidden">
              <div
                className="bg-blue-500 h-2 rounded-full transition-all duration-300"
                style={{ width: pruneTotal > 0 ? `${(pruneCurrent / pruneTotal) * 100}%` : "0%" }}
              />
            </div>
            <div className="flex items-center justify-between">
              <p className="text-xs text-gray-500">
                {pruneElapsed < 60
                  ? `${pruneElapsed}s elapsed`
                  : `${Math.floor(pruneElapsed / 60)}m ${pruneElapsed % 60}s elapsed`}
              </p>
              {pruneTotal > 0 && (
                <p className="text-xs text-gray-500">
                  {pruneCurrent} / {pruneTotal} complete
                </p>
              )}
            </div>
          </div>
        )}
      </Modal>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Browse Cache</h2>
        <p className="text-xs text-gray-500 mb-3">
          Snapshot file listings are cached locally to speed up navigation. Clear the cache if you
          see stale data or want to free up disk space.
        </p>
        <div className="flex items-center gap-3">
          <Button variant="secondary" onClick={handleClearCache} loading={clearingCache}>
            Clear Browse Cache
          </Button>
          {cacheCleared && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              Cleared
            </span>
          )}
        </div>
      </div>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5 text-center">
        <p className="text-xs text-gray-400">
          Made with love by{" "}
          <button
            onClick={() => open("https://www.nraboy.com")}
            className="text-blue-400 hover:underline"
          >
            Nic Raboy
          </button>{" "}
          in the United States.
        </p>
      </div>
    </div>
  );
}
