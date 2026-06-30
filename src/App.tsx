import { useEffect, useState, Component, type ReactNode, type ErrorInfo } from "react";
import { BrowserRouter, Routes, Route, useNavigate } from "react-router-dom";
import { open as openUrl } from "@tauri-apps/plugin-shell";
import { ThemeProvider } from "./lib/theme";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./components/Sidebar";
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
import AuthPage from "./pages/AuthPage";
import { isAppSetup, setupMasterPassword, unlockApp, setMenuAuthState, activateTray, getTrayEnabled, getResticVersion } from "./lib/invoke";
import { MIN_RESTIC_MAJOR, MIN_RESTIC_MINOR } from "./lib/config";

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

type AuthState = "loading" | "setup" | "locked" | "unlocked";

export default function App() {
  const [authState, setAuthState] = useState<AuthState>("loading");
  const [menuResetTriggered, setMenuResetTriggered] = useState(false);
  const [showVersionWarning, setShowVersionWarning] = useState(false);

  useEffect(() => {
    isAppSetup()
      .then((setup) => setAuthState(setup ? "locked" : "setup"))
      .catch(() => setAuthState("setup"));
  }, []);

  useEffect(() => {
    if (authState === "loading") return;
    setMenuAuthState(authState === "unlocked").catch(() => {});
    if (authState === "unlocked") {
      getTrayEnabled().then(enabled => { if (enabled) activateTray().catch(() => {}); }).catch(() => {});
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

  useEffect(() => {
    const unlisten = listen("menu:source-github", () => {
      openUrl("https://github.com/nraboy/resty-desktop");
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
          onSuccess={() => setAuthState("unlocked")}
          onSubmit={(password) => unlockApp(password)}
          onReset={() => setAuthState("setup")}
          openResetModal={menuResetTriggered}
          onResetModalOpened={() => setMenuResetTriggered(false)}
        />
      )}
      {authState === "unlocked" && (
        <BrowserRouter>
          <MenuEventHandler />
          <div className="flex h-screen w-screen overflow-hidden bg-gray-950">
            <Sidebar />
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
                    <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
                    </svg>
                  </button>
                </div>
              )}
              <main className="flex-1 overflow-y-auto">
                <ErrorBoundary>
                  <Routes>
                    <Route path="/" element={<RepositoriesPage />} />
                    <Route path="/snapshots/:repoId" element={<SnapshotsPage />} />
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
          </div>
        </BrowserRouter>
      )}
    </ThemeProvider>
  );
}
