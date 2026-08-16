import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-shell";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { activateTray, cancelPrune, changeMasterPassword, checkFullDiskAccess, cleanCache, clearBrowseCache, compressDatabase, deactivateTray, getAutoIndexing, getAutoUnlock, getAutoUnlockSupported, getCompression, getDbSize, getLaunchAtLogin, getLaunchAtLoginWarning, getNotificationSettings, getRemoteAutoRefresh, getResticPath, getResticVersion, getRestorePath, getTrayEnabled, getTrayWarning, listRepos, openFullDiskAccessSettings, pruneAllRepos, setAutoIndexing, setAutoUnlock, setCompression as saveCompression, setLaunchAtLogin, setNotificationSettings, setRemoteAutoRefresh, setResticPath, setRestorePath, setTrayEnabled } from "../lib/invoke";
import type { FullDiskAccessStatus } from "../lib/invoke";
import type { NotificationSettings } from "../lib/types";
import { formatBytes } from "../lib/format";
import { useTheme } from "../lib/theme";
import { useActivity } from "../lib/activity";
import type { Theme } from "../lib/theme";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import ImportExportCard from "../components/ImportExportCard";
import { ChevronDownIcon, CheckIcon, WarningIcon } from "../components/icons";

const DEFAULT_NOTIFICATION_SETTINGS: NotificationSettings = {
  started: false,
  successChanged: true,
  successUnchanged: false,
  failures: true,
};

const THEMES: { value: Theme; label: string; description: string }[] = [
  { value: "system", label: "System", description: "Follow the OS appearance" },
  { value: "light",  label: "Light",  description: "Always use the light theme" },
  { value: "dark",   label: "Dark",   description: "Always use the dark theme" },
];

export default function SettingsPage() {
  const { theme, setTheme } = useTheme();
  const { activePrune } = useActivity();
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
  const [compressing, setCompressing] = useState(false);
  const [compressed, setCompressed] = useState(false);
  const [dbSize, setDbSize] = useState<number | null>(null);
  // Shared by all three Application Cache buttons below — each had a try/finally with no
  // catch, so a failed clear/clean/compress was previously invisible.
  const [cacheOpError, setCacheOpError] = useState("");

  const [pruneModalOpen, setPruneModalOpen] = useState(false);
  const [pruneStarted, setPruneStarted] = useState(false);
  const [pruning, setPruning] = useState(false);
  const [pruneDone, setPruneDone] = useState(false);
  const [pruneCancelled, setPruneCancelled] = useState(false);
  const [pruneError, setPruneError] = useState("");
  const [pruneCurrent, setPruneCurrent] = useState(0);
  const [pruneTotal, setPruneTotal] = useState(0);
  const [pruneRepoName, setPruneRepoName] = useState("");
  const [pruneElapsed, setPruneElapsed] = useState(0);
  const [pruneStopping, setPruneStopping] = useState(false);
  // Loaded when the modal opens so the confirm/progress text can disclose that
  // prune_all_repos (repo.rs) silently skips read-only repos rather than pruning them —
  // otherwise "removes unreferenced data from every repository" would overpromise.
  const [readOnlyRepoCount, setReadOnlyRepoCount] = useState(0);
  const pruneStartRef = useRef<number>(0);
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cacheTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cleanTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const compressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const passwordTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [trayEnabled, setTrayEnabledLocal] = useState(false);
  const [trayWarning, setTrayWarning] = useState("");
  const [launchAtLogin, setLaunchAtLoginLocal] = useState(false);
  const [launchAtLoginWarning, setLaunchAtLoginWarning] = useState("");
  const [autoUnlock, setAutoUnlockLocal] = useState(false);
  const [autoUnlockSupported, setAutoUnlockSupported] = useState(false);
  const [autoIndexing, setAutoIndexingLocal] = useState(false);
  const [remoteAutoRefresh, setRemoteAutoRefreshLocal] = useState(false);
  const [notifications, setNotificationsLocal] = useState<NotificationSettings>(DEFAULT_NOTIFICATION_SETTINGS);
  // Mirrors `notifications` synchronously so updateNotifications can read/merge the latest value
  // without a functional setState updater — React.StrictMode (main.tsx) intentionally
  // double-invokes updater functions in dev, and the IPC save below must fire exactly once per
  // click, so the merge + side effect live in the plain event-handler body, not inside a setter.
  const notificationsRef = useRef(notifications);
  // Saves are full-object writes, so two independently in-flight saves can't be reconciled by
  // reverting individual fields (a later save's payload may already embed an earlier save's
  // change). These two refs serialize saves instead: at most one setNotificationSettings call is
  // ever in flight, and it always sends the latest merged state — see runNotificationsSaveLoop.
  const notificationsSavingRef = useRef(false);
  const notificationsDirtyRef = useRef(false);
  // One-way "user touched this" latch for the mount-effect load below. Unlike the two refs
  // above, it is never cleared once set — the saving/dirty flags both return to false once
  // runNotificationsSaveLoop drains, so a slow initial getNotificationSettings() resolving
  // *after* a completed save would still pass a guard on them and overwrite the just-persisted
  // state with the stale pre-change value. Once the user has clicked any checkbox, the save
  // loop is the sole source of truth for this card.
  const notificationsTouchedRef = useRef(false);

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
    getLaunchAtLogin().then(setLaunchAtLoginLocal).catch(() => {});
    getLaunchAtLoginWarning().then(setLaunchAtLoginWarning).catch(() => {});
    getAutoUnlock().then(setAutoUnlockLocal).catch(() => {});
    getAutoUnlockSupported().then(setAutoUnlockSupported).catch(() => {});
    getAutoIndexing().then(setAutoIndexingLocal).catch(() => {});
    getRemoteAutoRefresh().then(setRemoteAutoRefreshLocal).catch(() => {});
    getNotificationSettings().then((v) => {
      // Discard this stale read if the user already changed something before it resolved —
      // applying it here would clobber whatever the user set, and the save loop that change
      // triggered already reflects it correctly.
      if (notificationsTouchedRef.current) return;
      notificationsRef.current = v;
      setNotificationsLocal(v);
    }).catch(() => {});
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
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current);
      if (cacheTimerRef.current !== null) clearTimeout(cacheTimerRef.current);
      if (cleanTimerRef.current !== null) clearTimeout(cleanTimerRef.current);
      if (compressTimerRef.current !== null) clearTimeout(compressTimerRef.current);
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
    try {
      await setTrayEnabled(enabled);
      if (enabled) {
        // SettingsPage is only reachable while unlocked.
        await activateTray(true);
      } else {
        await deactivateTray();
      }
    } catch (err: any) {
      setTrayEnabledLocal(!enabled);
      setError(String(err));
      return;
    }
    // Cleared on BOTH transitions, not just the disable path: turning the tray off
    // unregisters the login item so no orphan is left behind that Settings can no
    // longer show; turning it on guarantees the now-interactive autostart toggle
    // starts from off rather than inheriting a surviving OS entry.
    //
    // Deliberately outside the try above: by this point the tray setting is persisted
    // and the tray icon is already created/removed, so rolling trayEnabled back on a
    // failure here would make the UI contradict both the DB and the real tray.
    try {
      await setLaunchAtLogin(false);
      setLaunchAtLoginLocal(false);
    } catch (err: any) {
      setError(String(err));
      // Don't assume "off" — the OS entry may well still exist. Re-read it so the
      // toggle keeps matching reality, which is the whole reason this setting has no
      // app_settings row.
      getLaunchAtLogin().then(setLaunchAtLoginLocal).catch(() => {});
    }
  };

  // Runs at most one setNotificationSettings call at a time, always sending the latest merged
  // state. A per-field local revert on failure can't be made correct here — saves send the whole
  // object, so a later save's payload can already embed an earlier save's change, and reverting
  // that field locally would then contradict what the later save actually persisted. Resyncing
  // from the server on failure sidesteps that entirely: it's correct regardless of what any other
  // in-flight or already-committed save did.
  const runNotificationsSaveLoop = async () => {
    if (notificationsSavingRef.current) return;
    notificationsSavingRef.current = true;
    try {
      while (notificationsDirtyRef.current) {
        notificationsDirtyRef.current = false;
        const snapshot = notificationsRef.current;
        try {
          await setNotificationSettings(snapshot);
        } catch (err: any) {
          setError(String(err));
          // Only resync if nothing newer is already queued (e.g. the user made another change
          // while this save was in flight, setting the flag back to true) — otherwise the loop's
          // next iteration will resend that fresher state anyway, and overwriting
          // notificationsRef with the server's (older) value here would silently discard it.
          if (!notificationsDirtyRef.current) {
            try {
              const server = await getNotificationSettings();
              // Re-check after this await too — a change can just as easily land during the
              // resync fetch itself, not only during the save above.
              if (!notificationsDirtyRef.current) {
                notificationsRef.current = server;
                setNotificationsLocal(server);
              }
            } catch {
              // Best effort — leave the optimistic UI as-is if the resync read itself fails.
            }
          }
        }
      }
    } finally {
      notificationsSavingRef.current = false;
    }
  };

  // Merges against notificationsRef (not a functional setState updater — React.StrictMode
  // double-invokes those in dev, which would fire the IPC save twice per click) so two
  // checkboxes clicked in quick succession each build on the other's just-applied change rather
  // than a stale closure. The actual save is handed off to runNotificationsSaveLoop, which
  // serializes it against any other in-flight save.
  const updateNotifications = (patch: Partial<NotificationSettings>) => {
    notificationsTouchedRef.current = true;
    const next = { ...notificationsRef.current, ...patch };
    notificationsRef.current = next;
    notificationsDirtyRef.current = true;
    setNotificationsLocal(next);
    void runNotificationsSaveLoop();
  };

  const handleClearCache = async () => {
    setClearingCache(true);
    setCacheOpError("");
    try {
      const newSize = await clearBrowseCache();
      setDbSize(newSize);
      setCacheCleared(true);
      if (cacheTimerRef.current !== null) clearTimeout(cacheTimerRef.current);
      cacheTimerRef.current = setTimeout(() => setCacheCleared(false), 2000);
    } catch (err: any) {
      setCacheOpError(String(err));
    } finally {
      setClearingCache(false);
    }
  };

  const handleCleanCache = async () => {
    setCleaningCache(true);
    setCacheOpError("");
    try {
      const [removed, newSize] = await cleanCache();
      setCleanedCount(removed);
      setDbSize(newSize);
      if (cleanTimerRef.current !== null) clearTimeout(cleanTimerRef.current);
      cleanTimerRef.current = setTimeout(() => setCleanedCount(null), 4000);
    } catch (err: any) {
      setCacheOpError(String(err));
    } finally {
      setCleaningCache(false);
    }
  };

  const handleCompressDatabase = async () => {
    setCompressing(true);
    setCacheOpError("");
    try {
      const newSize = await compressDatabase();
      setDbSize(newSize);
      setCompressed(true);
      if (compressTimerRef.current !== null) clearTimeout(compressTimerRef.current);
      compressTimerRef.current = setTimeout(() => setCompressed(false), 2000);
    } catch (err: any) {
      setCacheOpError(String(err));
    } finally {
      setCompressing(false);
    }
  };

  const handlePruneAll = async () => {
    setPruneStarted(true);
    setPruning(true);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    setPruneCurrent(0);
    setPruneTotal(0);
    setPruneRepoName("");
    setPruneStopping(false);

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
    }
  };

  // Mirrors the shared `activePrune` task-bus slot (see activity.tsx) into this modal's local
  // display state while this modal's own run is in flight. Gated on `pruning` — activePrune is a
  // single app-wide slot (prune is single-in-flight via PruneHandle's busy guard), so without this
  // gate a per-repo prune started elsewhere (RepositoriesPage's context-menu Prune, which shares
  // the same slot but never carries progress — itemsTotal stays 0) could overwrite this modal's
  // numbers if it happened to be open. `pruning` stays true until handlePruneAll's `finally`,
  // i.e. until after the task's own `finished`/`failed`/`cancelled`, so the last per-repo tick
  // (itemsDone/itemsTotal at the final repo) is captured before activePrune clears to null —
  // the done screen below still reads the correct total.
  useEffect(() => {
    if (pruning && activePrune) {
      setPruneCurrent(activePrune.itemsDone);
      setPruneTotal(activePrune.itemsTotal);
      setPruneRepoName(activePrune.repoLabel ?? "");
    }
  }, [pruning, activePrune]);

  // Reset the modal's display state (not the operation itself) — called before opening the
  // modal for a fresh run so a previously-dismissed run's done/error/cancelled screen doesn't
  // reappear. Never called while `pruning` is true (see the open button below), since a
  // still-running prune should reopen straight into its live progress, not a blank state.
  const resetPruneDisplay = () => {
    setPruneStarted(false);
    setPruneDone(false);
    setPruneCancelled(false);
    setPruneError("");
    setPruneCurrent(0);
    setPruneTotal(0);
    setPruneRepoName("");
  };

  // Hide the modal only. Previously this cancelled the prune on close (or on navigating away,
  // since unmount ran the same path) — now the prune keeps running and stays visible/cancellable
  // via the Activity panel's activePrune row (see activity.tsx), which this modal itself mirrors
  // (see the pruning-gated effect above `handlePruneAll`) — so reopening the modal mid-run shows
  // live progress instead of a blank state, sourced straight from the shared task-bus state.
  const closePruneModal = () => {
    setPruneModalOpen(false);
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
              When enabled, closing the window keeps the app running in the system tray instead of
              quitting — scheduled backups run whenever the app is unlocked, tray icon or not. A
              locked app shows "Locked" in the tray menu until you unlock it.
            </p>
            <label className="flex items-center gap-3 cursor-pointer select-none">
              <button
                role="switch"
                aria-checked={trayEnabled}
                aria-label="Keep app running in tray when window is closed"
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
          <div className={["pt-4 border-t border-gray-800", trayEnabled ? "" : "opacity-50"].join(" ")}>
            <p className="text-xs text-gray-500 mb-3">
              {autoUnlockSupported && autoUnlock
                ? "Start Resty Desktop automatically when you log in. It starts hidden in the tray and scheduled backups resume immediately — open it from the tray icon."
                : "Start Resty Desktop automatically when you log in. The app opens to the unlock screen; scheduled backups resume once you unlock it."}
            </p>
            <label
              className={[
                "flex items-center gap-3 select-none",
                trayEnabled ? "cursor-pointer" : "cursor-default",
              ].join(" ")}
            >
              <button
                role="switch"
                aria-checked={trayEnabled && launchAtLogin}
                aria-label="Start Resty Desktop at login"
                disabled={!trayEnabled}
                onClick={() => {
                  const next = !launchAtLogin;
                  setLaunchAtLoginLocal(next);
                  setLaunchAtLogin(next).catch((err) => {
                    setLaunchAtLoginLocal(!next);
                    setError(String(err));
                  });
                }}
                className={[
                  "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900",
                  trayEnabled && launchAtLogin ? "bg-blue-600" : "bg-gray-700",
                  trayEnabled ? "" : "cursor-not-allowed",
                ].join(" ")}
              >
                <span
                  className={[
                    "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                    trayEnabled && launchAtLogin ? "translate-x-4" : "translate-x-1",
                  ].join(" ")}
                />
              </button>
              <span className="text-sm text-gray-300">Start Resty Desktop at login</span>
            </label>
            {!trayEnabled && (
              <p className="mt-3 text-xs text-gray-500">
                Requires the tray setting above — without it, closing the window quits the app.
              </p>
            )}
            {trayEnabled && launchAtLoginWarning && (
              <p className="mt-3 text-xs text-amber-500">{launchAtLoginWarning}</p>
            )}
          </div>
          {autoUnlockSupported && (
            <div className="pt-4 border-t border-gray-800">
              <p className="text-xs text-gray-500 mb-3">
                Your master key is stored in your system's credential manager so Resty can unlock
                itself at startup — including scheduled backups after a login launch. Anyone who
                can log into this computer will be able to use your repositories without the
                master password.
              </p>
              <label className="flex items-center gap-3 cursor-pointer select-none">
                <button
                  role="switch"
                  aria-checked={autoUnlock}
                  aria-label="Unlock automatically at startup"
                  onClick={() => {
                    const next = !autoUnlock;
                    setAutoUnlockLocal(next);
                    setAutoUnlock(next).catch((err) => {
                      // Disabling always clears the row server-side even if the keychain
                      // delete itself failed (see set_auto_unlock's doc comment in auth.rs)
                      // — so a failed disable must keep the toggle OFF to match reality.
                      // Only a failed enable leaves the row untouched and needs reverting.
                      if (next) {
                        setAutoUnlockLocal(false);
                      }
                      setError(String(err));
                    });
                  }}
                  className={[
                    "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900",
                    autoUnlock ? "bg-blue-600" : "bg-gray-700",
                  ].join(" ")}
                >
                  <span
                    className={[
                      "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                      autoUnlock ? "translate-x-4" : "translate-x-1",
                    ].join(" ")}
                  />
                </button>
                <span className="text-sm text-gray-300">Unlock automatically at startup</span>
              </label>
            </div>
          )}
          <div className="pt-4 border-t border-gray-800">
            <p className="text-xs text-gray-500 mb-3">
              When enabled, the background cache warmer pre-indexes file listings for every snapshot so
              browsing is instant. When disabled, file listings are still cached on-demand the first time
              you browse a snapshot. Snapshot metadata is always kept up to date regardless of this setting.
            </p>
            <label className="flex items-center gap-3 cursor-pointer select-none">
              <button
                role="switch"
                aria-checked={autoIndexing}
                aria-label="Automatic background file indexing"
                onClick={() => {
                  const next = !autoIndexing;
                  setAutoIndexingLocal(next);
                  setAutoIndexing(next).catch((err) => {
                    setAutoIndexingLocal(!next);
                    setError(String(err));
                  });
                }}
                className={[
                  "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-gray-900",
                  autoIndexing ? "bg-blue-600" : "bg-gray-700",
                ].join(" ")}
              >
                <span
                  className={[
                    "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                    autoIndexing ? "translate-x-4" : "translate-x-1",
                  ].join(" ")}
                />
              </button>
              <span className="text-sm text-gray-300">Automatic background file indexing</span>
            </label>
          </div>
          <div className="pt-4 border-t border-gray-800">
            <p className="text-xs text-gray-500 mb-3">
              When enabled, remote repositories are refreshed and cached automatically — on page load and
              in the background — the same as local repositories. Disabled by default to avoid
              unnecessary bandwidth charges from your cloud provider. Manual stats refresh (the Refresh
              buttons on the Repositories page) always includes remote repositories regardless of this
              setting, since that's an explicit, user-initiated request rather than an automatic one.
            </p>
            <label className="flex items-center gap-3 cursor-pointer select-none">
              <button
                role="switch"
                aria-checked={remoteAutoRefresh}
                aria-label="Auto-refresh data for remote repositories"
                onClick={() => {
                  const next = !remoteAutoRefresh;
                  setRemoteAutoRefreshLocal(next);
                  setRemoteAutoRefresh(next).catch((err) => {
                    setRemoteAutoRefreshLocal(!next);
                    setError(String(err));
                  });
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

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Notifications</h2>
        <p className="text-xs text-gray-500 mb-4">
          Choose which desktop notifications appear for backups — manual and scheduled alike.
          Muting a category only hides the notification; the backup still shows up in Recent Logs
          and the Activity panel either way.
        </p>
        <div className="grid grid-cols-3 gap-x-6 gap-y-2">
          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              className="w-4 h-4 accent-blue-500"
              checked={notifications.started}
              onChange={(e) => updateNotifications({ started: e.target.checked })}
            />
            Backup started
          </label>
          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              className="w-4 h-4 accent-blue-500"
              checked={notifications.successChanged}
              onChange={(e) => updateNotifications({ successChanged: e.target.checked })}
            />
            Success (files changed)
          </label>
          <label className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              className="w-4 h-4 accent-blue-500"
              checked={notifications.successUnchanged}
              onChange={(e) => updateNotifications({ successUnchanged: e.target.checked })}
            />
            Success (no files changed)
          </label>
          <label
            className="flex items-center gap-2 text-sm text-gray-400 cursor-pointer"
            title="Includes cancelled backups."
          >
            <input
              type="checkbox"
              className="w-4 h-4 accent-blue-500"
              checked={notifications.failures}
              onChange={(e) => updateNotifications({ failures: e.target.checked })}
            />
            Failures
          </label>
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
            <ChevronDownIcon className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-gray-400" />
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
              <CheckIcon className="w-4 h-4" />
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
              <CheckIcon className="w-4 h-4 text-green-400 flex-shrink-0" />
              <span className="text-sm text-green-400">Full Disk Access is enabled.</span>
            </div>
          ) : (
            <div className="flex items-start gap-2 mb-3 p-3 bg-amber-900/40 border border-amber-700/50 rounded-lg">
              <WarningIcon className="w-4 h-4 text-amber-400 flex-shrink-0 mt-0.5" />
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
                <CheckIcon className="w-4 h-4" />
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
        <Button
          variant="secondary"
          onClick={() => {
            // Only reset the modal's display to the confirm screen when nothing is running —
            // a still-running prune (survived a prior dismiss) should reopen into its live
            // progress, not a blank confirm screen.
            if (!pruning) resetPruneDisplay();
            listRepos()
              .then((repos) => setReadOnlyRepoCount(repos.filter((r) => r.readOnly).length))
              .catch(() => setReadOnlyRepoCount(0));
            setPruneModalOpen(true);
          }}
        >
          Prune All Repositories
        </Button>
      </div>

      <Modal open={pruneModalOpen} onClose={closePruneModal} title="Prune All Repositories">
        {!pruneStarted ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">
              This permanently removes unreferenced data from every writable repository — pack
              files not tied to any snapshot. It cannot be undone.
            </p>
            {readOnlyRepoCount > 0 && (
              <p className="text-sm text-amber-400">
                {readOnlyRepoCount} read-only {readOnlyRepoCount === 1 ? "repository is" : "repositories are"} excluded and will not be pruned.
              </p>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setPruneModalOpen(false)}>Cancel</Button>
              <Button variant="danger" onClick={handlePruneAll}>Prune All</Button>
            </div>
          </div>
        ) : pruneDone ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">
              All {pruneTotal} {pruneTotal === 1 ? "repository has" : "repositories have"} been pruned successfully.
            </p>
            {readOnlyRepoCount > 0 && (
              <p className="text-xs text-gray-500">
                {readOnlyRepoCount} read-only {readOnlyRepoCount === 1 ? "repository was" : "repositories were"} skipped.
              </p>
            )}
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
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closePruneModal} title="Keep pruning in the background">
                Hide
              </Button>
              <Button
                variant="danger"
                disabled={pruneStopping}
                onClick={async () => {
                  setPruneStopping(true);
                  try {
                    await cancelPrune();
                  } catch {
                    // The cancel call itself failed (e.g. a transient IPC error) — the prune is
                    // still running untouched, so roll back rather than leaving Stop stuck
                    // disabled with no way to retry.
                    setPruneStopping(false);
                  }
                }}
              >
                {pruneStopping ? "Stopping…" : "Stop"}
              </Button>
            </div>
          </div>
        )}
      </Modal>

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Application Cache</h2>
        <p className="text-xs text-gray-500 mb-3">
          Snapshot listings and repository stats are cached locally to speed up navigation.
          <strong className="text-gray-400"> Clean Orphaned Data</strong> removes only orphaned entries left
          behind by deleted repositories and forgotten snapshots,
          <strong className="text-gray-400"> Clear All Cache</strong> wipes everything (rebuilt on
          next use), and
          <strong className="text-gray-400"> Compress Database</strong> reclaims disk space left
          behind by deleted rows without removing any data.
        </p>
        <div className="flex items-center gap-3">
          <Button variant="secondary" onClick={handleCleanCache} loading={cleaningCache}>
            Clean Orphaned Data
          </Button>
          <Button variant="secondary" onClick={handleClearCache} loading={clearingCache}>
            Clear All Cache
          </Button>
          <Button variant="secondary" onClick={handleCompressDatabase} loading={compressing}>
            Compress Database
          </Button>
          {cleanedCount !== null && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <CheckIcon className="w-4 h-4" />
              {cleanedCount === 0
                ? "No orphaned entries"
                : `Removed ${cleanedCount} orphaned ${cleanedCount === 1 ? "entry" : "entries"}`}
            </span>
          )}
          {cacheCleared && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <CheckIcon className="w-4 h-4" />
              Cleared
            </span>
          )}
          {compressed && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <CheckIcon className="w-4 h-4" />
              Compressed
            </span>
          )}
        </div>
        {cacheOpError && <p className="text-sm text-red-300 mt-3">{cacheOpError}</p>}
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
