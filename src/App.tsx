import { useCallback, useEffect, useRef, useState, Component, type ReactNode, type ErrorInfo } from "react";
import { BrowserRouter, Routes, Route, useNavigate } from "react-router-dom";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { ThemeProvider } from "./lib/theme";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./components/Sidebar";
import ActivityPanel from "./components/ActivityPanel";
import Button from "./components/Button";
import { XIcon } from "./components/icons";
import { ActivityProvider } from "./lib/activity";
import RepositoriesPage from "./pages/RepositoriesPage";
import SnapshotsPage from "./pages/SnapshotsPage";
import BrowsePage from "./pages/BrowsePage";
import DiffPage from "./pages/DiffPage";
import BackupPlansPage from "./pages/BackupPlansPage";
import BackupPlanEditPage from "./pages/BackupPlanEditPage";
import SchedulesPage from "./pages/SchedulesPage";
import ScheduleEditPage from "./pages/ScheduleEditPage";
import SettingsPage from "./pages/SettingsPage";
import LogsPage from "./pages/LogsPage";
import SearchPage from "./pages/SearchPage";
import RepoSearchPage from "./pages/RepoSearchPage";
import AuthPage from "./pages/AuthPage";
import { isAppSetup, setupMasterPassword, unlockApp, lockApp, tryAutoUnlock, autoUnlockNeedsPromptWarning, setMenuAuthState, activateTray, deactivateTray, showMainWindow, getTrayEnabled, getResticVersion } from "./lib/invoke";
import { MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR } from "./lib/config";
import type { TaskEvent } from "./lib/types";

// Reason codes returned by try_auto_unlock, mapped to display copy here — the Rust side only
// ever returns a machine-readable code (see AutoUnlockResult in lib/types.ts), never English.
const AUTO_UNLOCK_NOTICES: Record<string, string> = {
  denied: "Automatic unlock couldn't access your keychain. Enter your master password to continue.",
  stale: "Your saved key is no longer valid and has been removed. Enter your master password to continue.",
};

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Unhandled render error:", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="flex flex-col items-center justify-center h-screen w-screen bg-gray-950 gap-4 p-8">
          <p className="text-gray-100 font-semibold">Something went wrong</p>
          <p className="text-gray-400 text-sm text-center max-w-md">{this.state.error.message}</p>
          <button
            className="text-blue-400 text-sm hover:underline"
            onClick={() => this.setState({ error: null })}
          >
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

function MenuEventHandler() {
  const navigate = useNavigate();
  useEffect(() => {
    const unlistenNewRepo = listen("menu:new-repository", () => {
      navigate("/?action=new-repo");
    });
    const unlistenNewPlan = listen("menu:new-backup-plan", () => {
      navigate("/backup-plans/new");
    });
    const unlistenSettings = listen("menu:settings", () => {
      navigate("/settings");
    });
    const unlistenImport = listen("menu:import", () => {
      navigate("/settings?action=import");
    });
    const unlistenExport = listen("menu:export", () => {
      navigate("/settings?action=export");
    });
    return () => {
      unlistenNewRepo.then((fn) => fn());
      unlistenNewPlan.then((fn) => fn());
      unlistenSettings.then((fn) => fn());
      unlistenImport.then((fn) => fn());
      unlistenExport.then((fn) => fn());
    };
  }, [navigate]);
  return null;
}

type AuthState = "loading" | "setup" | "locked" | "unlocked" | "updateNotice";

export default function App() {
  const [authState, setAuthState] = useState<AuthState>("loading");
  const [menuResetTriggered, setMenuResetTriggered] = useState(false);
  const [showVersionWarning, setShowVersionWarning] = useState(false);
  const [autoUnlockReason, setAutoUnlockReason] = useState("");
  // Guards the startup effect below against React StrictMode's deliberate double-invoke of
  // effects in development — without this, tryAutoUnlock() (and the real macOS keychain
  // authorization it can trigger) would fire twice per dev-mode launch. Refs survive
  // StrictMode's simulated mount/cleanup/remount cycle (only the effect body re-runs), so this
  // reliably limits the startup sequence to one real attempt. No effect in production, where
  // React only invokes effects once.
  const startupRanRef = useRef(false);
  // Tracks whether this session has ever reached "unlocked". A ref, not state, so setting it
  // doesn't itself trigger the effect below. Distinguishes a hidden login launch that hasn't
  // succeeded yet (auto-unlock still pending, or failed/denied/stale, or updateNotice) — which
  // must still force the window visible — from a deliberate mid-session lock via the tray's
  // or menu bar's "Lock Now" (see lib.rs's tray_lock_{gen} item), which must NOT. Without this,
  // locking from the tray while hidden would immediately pop the window back open.
  const hasBeenUnlockedRef = useRef(false);
  // Whether the Activity panel (ActivityPanel) is shown — owned here so the Sidebar's footer
  // status strip drives it. A transient overlay only: nothing is persisted, and locking unmounts
  // the unlocked branch below, resetting it to closed.
  const [activityOpen, setActivityOpen] = useState(false);
  // Stable identities so ActivityPanel's outside-close effect (deps [open, onClose]) doesn't
  // re-subscribe its document listener on every App render.
  const closeActivity = useCallback(() => setActivityOpen(false), []);
  const toggleActivity = useCallback(() => setActivityOpen((v) => !v), []);

  // Shared by the mount effect below and the "updateNotice" screen's Continue button, so both
  // paths land on the exact same success/failure handling.
  const runAutoUnlock = useCallback(() => {
    tryAutoUnlock()
      .then((result) => {
        if (result.unlocked) {
          setAutoUnlockReason("");
          setAuthState("unlocked");
        } else {
          setAutoUnlockReason(result.reason);
          setAuthState("locked");
        }
      })
      .catch(() => {
        // try_auto_unlock itself is designed to never reject (see its doc comment in
        // auth.rs) — this only catches something unexpected, e.g. the IPC call failing
        // outright. Fail safe to the manual unlock screen either way.
        setAutoUnlockReason("denied");
        setAuthState("locked");
      });
  }, []);

  useEffect(() => {
    if (startupRanRef.current) return;
    startupRanRef.current = true;
    isAppSetup()
      .then((setup) => {
        if (!setup) {
          setAuthState("setup");
          return;
        }
        autoUnlockNeedsPromptWarning()
          .then((needsWarning) => {
            if (needsWarning) {
              setAuthState("updateNotice");
            } else {
              runAutoUnlock();
            }
          })
          .catch(() => runAutoUnlock());
      })
      .catch(() => setAuthState("setup"));
  }, [runAutoUnlock]);

  useEffect(() => {
    if (authState === "loading") return;
    setMenuAuthState(authState === "unlocked").catch(() => {});

    if (authState === "unlocked") {
      hasBeenUnlockedRef.current = true;
    } else if (!hasBeenUnlockedRef.current) {
      // Rescues a hidden login launch whose auto-unlock failed ("denied"/"stale") or that
      // needs the macOS post-update keychain prompt ("updateNotice") — but only before the
      // session has ever unlocked. Once it has, "not unlocked" instead means a deliberate
      // Lock Now from the tray, which must leave a hidden window hidden. It's a no-op on an
      // already-visible window either way, so no started-hidden flag has to be plumbed in.
      showMainWindow().catch(() => {});
    }

    if (authState === "setup") {
      // Reaching "setup" after startup means a reset: reset_all wipes app_settings (so
      // tray_enabled reverts to its false default) but not the tray icon setup() already
      // built for the app's previous lifetime.
      deactivateTray().catch(() => {});
    } else {
      getTrayEnabled()
        .then((enabled) => { if (enabled) activateTray(authState === "unlocked").catch(() => {}); })
        .catch(() => {});
    }

    if (authState === "unlocked") {
      getResticVersion().then((v) => {
        const m = v.match(/restic (\d+)\.(\d+)/);
        if (m) {
          const [major, minor] = [parseInt(m[1]), parseInt(m[2])];
          if (major === MIN_RESTIC_MAJOR && minor < MIN_RESTIC_MINOR) setShowVersionWarning(true);
        }
      }).catch(() => {});
    }
  }, [authState]);

  useEffect(() => {
    if (authState !== "locked") return;
    const unlisten = listen("menu:reset-app", () => setMenuResetTriggered(true));
    return () => { unlisten.then((fn) => fn()); };
  }, [authState]);

  // Shared by the menu-bar/tray "Lock Now" listener below and the sidebar's lock button —
  // the only two UI paths that lock a session (the tray one exists only when the tray
  // setting is on, which is why the sidebar button matters on Windows/Linux where there's
  // no native menu bar).
  const handleLock = useCallback(() => {
    // Locking is a session action, not a settings change — it never touches the keychain, so
    // an auto-unlock user lands right back in the unlocked app on next launch, which is correct.
    lockApp()
      .then(() => { setAutoUnlockReason(""); setAuthState("locked"); })
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (authState !== "unlocked") return;
    const unlisten = listen("menu:lock-app", handleLock);
    return () => { unlisten.then((fn) => fn()); };
  }, [authState, handleLock]);

  useEffect(() => {
    const unlisten = listen("menu:source-github", () => {
      openUrl("https://github.com/nraboy/resty-desktop");
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  // Dev-only visibility into the unified operation event bus (see tasks.rs /
  // CLAUDE.md's "Operation Event Bus" section). Deliberately stateless — never
  // calls setState — so it can never cause a re-render; nothing in the app
  // reads this data yet. Safe to remove; devtools' own event inspector works
  // just as well, this is just a convenience during development.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    const unlisten = listen<TaskEvent>("task", (event) => {
      console.debug("[task]", event.payload);
    });
    return () => { unlisten.then((fn) => fn()); };
  }, []);

  return (
    <ThemeProvider>
      {authState === "loading" && (
        <div className="flex items-center justify-center h-screen w-screen bg-gray-950">
          <p className="text-gray-500 text-sm">Loading…</p>
        </div>
      )}
      {authState === "updateNotice" && (
        <div className="flex items-center justify-center h-screen w-screen bg-gray-950">
          <div className="w-full max-w-sm px-6 text-center">
            <h1 className="text-xl font-semibold text-gray-100 mb-2">Resty Desktop was updated</h1>
            <p className="text-sm text-gray-400 mb-6">
              macOS will ask for permission to read your saved key. Choose{" "}
              <span className="font-semibold text-gray-200">Always Allow</span> so you aren't
              asked again until the next update.
            </p>
            <Button onClick={runAutoUnlock} className="w-full justify-center">
              Continue
            </Button>
          </div>
        </div>
      )}
      {authState === "setup" && (
        <AuthPage
          mode="setup"
          onSuccess={() => setAuthState("unlocked")}
          onSubmit={setupMasterPassword}
        />
      )}
      {authState === "locked" && (
        <AuthPage
          mode="unlock"
          notice={AUTO_UNLOCK_NOTICES[autoUnlockReason]}
          onSuccess={() => { setAutoUnlockReason(""); setAuthState("unlocked"); }}
          onSubmit={(password) => unlockApp(password)}
          onReset={() => setAuthState("setup")}
          openResetModal={menuResetTriggered}
          onResetModalOpened={() => setMenuResetTriggered(false)}
        />
      )}
      {authState === "unlocked" && (
        <BrowserRouter>
          <MenuEventHandler />
          <ActivityProvider>
            <div className="flex h-screen w-screen overflow-hidden bg-gray-950">
              <Sidebar onLock={handleLock} activityOpen={activityOpen} onToggleActivity={toggleActivity} />
              <div className="flex-1 flex flex-col overflow-hidden">
                {showVersionWarning && (
                  <div className="flex items-center justify-between gap-3 px-4 py-2.5 bg-yellow-900/50 border-b border-yellow-700 text-yellow-200 text-sm flex-shrink-0">
                    <span>
                      For the best experience, upgrade to <strong>restic {MIN_RESTIC_MAJOR}.{MIN_RESTIC_MINOR} or newer</strong>. Some retention and grouping features may not work correctly on older versions.
                    </span>
                    <button
                      onClick={() => setShowVersionWarning(false)}
                      className="flex-shrink-0 text-yellow-300 hover:text-yellow-100 transition-colors"
                      aria-label="Dismiss"
                    >
                      <XIcon className="w-4 h-4" />
                    </button>
                  </div>
                )}
                <main className="flex-1 overflow-y-auto">
                  <ErrorBoundary>
                    <Routes>
                      <Route path="/" element={<RepositoriesPage />} />
                      <Route path="/snapshots/:repoId" element={<SnapshotsPage />} />
                      <Route path="/snapshots/:repoId/search" element={<RepoSearchPage />} />
                      <Route path="/snapshots/:repoId/:snapshotId/browse" element={<BrowsePage />} />
                      <Route path="/snapshots/:repoId/:snapshotId/search" element={<SearchPage />} />
                      <Route path="/snapshots/:repoId/diff/:snapshotA/:snapshotB" element={<DiffPage />} />
                      <Route path="/backup-plans" element={<BackupPlansPage />} />
                      <Route path="/backup-plans/:planId" element={<BackupPlanEditPage />} />
                      <Route path="/schedules" element={<SchedulesPage />} />
                      <Route path="/schedules/:scheduleId" element={<ScheduleEditPage />} />
                      <Route path="/logs" element={<LogsPage />} />
                      <Route path="/settings" element={<SettingsPage />} />
                    </Routes>
                  </ErrorBoundary>
                </main>
              </div>
              <ActivityPanel open={activityOpen} onClose={closeActivity} />
            </div>
          </ActivityProvider>
        </BrowserRouter>
      )}
    </ThemeProvider>
  );
}
