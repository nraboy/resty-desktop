import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-shell";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { activateTray, cancelPrune, changeMasterPassword, checkFullDiskAccess, cleanCache, clearBrowseCache, deactivateTray, getCompression, getDbSize, getRemoteAutoRefresh, getResticPath, getResticVersion, getRestorePath, getTrayEnabled, getTrayWarning, openFullDiskAccessSettings, pruneAllRepos, setCompression as saveCompression, setRemoteAutoRefresh, setResticPath, setRestorePath, setTrayEnabled } from "../lib/invoke";
import type { FullDiskAccessStatus } from "../lib/invoke";
import { formatBytes } from "../lib/format";
import { useTheme } from "../lib/theme";
import type { Theme } from "../lib/theme";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import ImportExportCard from "../components/ImportExportCard";

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
  const [cleaningCache, setCleaningCache] = useState(false);
  const [cleanedCount, setCleanedCount] = useState<number | null>(null);
  const [dbSize, setDbSize] = useState<number | null>(null);

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
  const cleanTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const passwordTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [trayEnabled, setTrayEnabledLocal] = useState(false);
  const [trayWarning, setTrayWarning] = useState("");
  const [remoteAutoRefresh, setRemoteAutoRefreshLocal] = useState(false);

  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [changingPassword, setChangingPassword] = useState(false);
  const [passwordChanged, setPasswordChanged] = useState(false);
  const [passwordError, setPasswordError] = useState("");

  const [fdaStatus, setFdaStatus] = useState<FullDiskAccessStatus | null>(null);
  const [fdaChecking, setFdaChecking] = useState(false);

  useEffect(() => {
    getResticPath().then(setResticPathLocal).catch(() => {});
    getCompression().then(setCompression).catch(() => {});
    getRestorePath().then(setRestorePathLocal).catch(() => {});
    getTrayEnabled().then(setTrayEnabledLocal).catch(() => {});
    getTrayWarning().then(setTrayWarning).catch(() => {});
    getRemoteAutoRefresh().then(setRemoteAutoRefreshLocal).catch(() => {});
    getResticVersion()
      .then((v) => { setResticVersion(v); setVersionError(""); })
      .catch((e) => { setResticVersion(null); setVersionError(String(e)); });
    checkFullDiskAccess().then(setFdaStatus).catch(() => {});
    getDbSize().then(setDbSize).catch(() => {});
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
      if (cleanTimerRef.current !== null) clearTimeout(cleanTimerRef.current);
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

  const handleCleanCache = async () => {
    setCleaningCache(true);
    try {
      const removed = await cleanCache();
      setCleanedCount(removed);
      if (cleanTimerRef.current !== null) clearTimeout(cleanTimerRef.current);
      cleanTimerRef.current = setTimeout(() => setCleanedCount(null), 4000);
    } finally {
      setCleaningCache(false);
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

  const handleFdaOpen = async () => {
    await openFullDiskAccessSettings().catch(() => {});
  };

  const handleFdaRecheck = async () => {
    setFdaChecking(true);
    try {
      const status = await checkFullDiskAccess();
      setFdaStatus(status);
    } finally {
      setFdaChecking(false);
    }
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
            {trayWarning && (
              <p className="mt-3 text-xs text-amber-500">{trayWarning}</p>
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

      {fdaStatus?.supported && (
        <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
          <h2 className="text-sm font-medium text-gray-300 mb-1">Full Disk Access</h2>
          <p className="text-xs text-gray-500 mb-3">
            Backing up protected directories like <code className="text-gray-400">~/Library</code>,{" "}
            <code className="text-gray-400">/System</code>, and <code className="text-gray-400">/private</code>{" "}
            requires Full Disk Access. Without it, restic will encounter permission errors on those paths.
            Note: after an app update, macOS may revoke this grant and you'll need to re-add Resty Desktop.
          </p>
          {fdaStatus.granted ? (
            <div className="flex items-center gap-2 mb-3">
              <svg className="w-4 h-4 text-green-400 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              <span className="text-sm text-green-400">Full Disk Access is enabled.</span>
            </div>
          ) : (
            <div className="flex items-start gap-2 mb-3 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg">
              <svg className="w-4 h-4 text-amber-400 flex-shrink-0 mt-0.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01M10.29 3.86L1.82 18a2 2 0 001.71 3h16.94a2 2 0 001.71-3L13.71 3.86a2 2 0 00-3.42 0z" />
              </svg>
              <p className="text-xs text-amber-300">
                <span className="font-medium">Full Disk Access is not enabled.</span>{" "}
                Open <span className="font-medium">System Settings → Privacy &amp; Security → Full Disk Access</span>{" "}
                and add Resty Desktop to avoid permission errors when backing up protected directories.
              </p>
            </div>
          )}
          <div className="flex items-center gap-3">
            <Button variant="secondary" onClick={handleFdaOpen}>
              Open Full Disk Access Settings
            </Button>
            <Button variant="secondary" onClick={handleFdaRecheck} loading={fdaChecking}>
              Re-check
            </Button>
          </div>
        </div>
      )}

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

      <ImportExportCard />

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
        <h2 className="text-sm font-medium text-gray-300 mb-1">Application Cache</h2>
        <p className="text-xs text-gray-500 mb-3">
          Snapshot listings and repository stats are cached locally to speed up navigation.
          <strong className="text-gray-400"> Clean Cache</strong> removes only orphaned entries left
          behind by deleted repositories and forgotten snapshots, while
          <strong className="text-gray-400"> Clear All Cache</strong> wipes everything (rebuilt on
          next use).
        </p>
        <div className="flex items-center gap-3">
          <Button variant="secondary" onClick={handleCleanCache} loading={cleaningCache}>
            Clean Cache
          </Button>
          <Button variant="secondary" onClick={handleClearCache} loading={clearingCache}>
            Clear All Cache
          </Button>
          {cleanedCount !== null && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              {cleanedCount === 0
                ? "No orphaned entries"
                : `Removed ${cleanedCount} orphaned ${cleanedCount === 1 ? "entry" : "entries"}`}
            </span>
          )}
          {cacheCleared && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              Cleared
            </span>
          )}
        </div>
        {dbSize !== null && (
          <p className="text-xs text-gray-500 mt-3">Current DB Size: {formatBytes(dbSize)}</p>
        )}
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
