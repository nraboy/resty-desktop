import { useEffect, useState } from "react";
import { BrowserRouter, Routes, Route, useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import Sidebar from "./components/Sidebar";
import RepositoriesPage from "./pages/RepositoriesPage";
import SnapshotsPage from "./pages/SnapshotsPage";
import BrowsePage from "./pages/BrowsePage";
import BackupPlansPage from "./pages/BackupPlansPage";
import BackupPlanEditPage from "./pages/BackupPlanEditPage";
import SchedulesPage from "./pages/SchedulesPage";
import ScheduleEditPage from "./pages/ScheduleEditPage";
import SettingsPage from "./pages/SettingsPage";
import LogsPage from "./pages/LogsPage";
import AuthPage from "./pages/AuthPage";
import { isAppSetup, setupMasterPassword, unlockApp, setMenuAuthState } from "./lib/invoke";

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
    return () => {
      unlistenNewRepo.then((fn) => fn());
      unlistenNewPlan.then((fn) => fn());
      unlistenSettings.then((fn) => fn());
    };
  }, [navigate]);
  return null;
}

type AuthState = "loading" | "setup" | "locked" | "unlocked";

export default function App() {
  const [authState, setAuthState] = useState<AuthState>("loading");
  const [menuResetTriggered, setMenuResetTriggered] = useState(false);

  useEffect(() => {
    isAppSetup()
      .then((setup) => setAuthState(setup ? "locked" : "setup"))
      .catch(() => setAuthState("setup"));
  }, []);

  useEffect(() => {
    if (authState === "loading") return;
    setMenuAuthState(authState === "unlocked").catch(() => {});
  }, [authState]);

  useEffect(() => {
    if (authState !== "locked") return;
    const unlisten = listen("menu:reset-app", () => setMenuResetTriggered(true));
    return () => { unlisten.then((fn) => fn()); };
  }, [authState]);

  if (authState === "loading") {
    return (
      <div className="flex items-center justify-center h-screen w-screen bg-gray-950">
        <p className="text-gray-500 text-sm">Loading…</p>
      </div>
    );
  }

  if (authState === "setup") {
    return (
      <AuthPage
        mode="setup"
        onSuccess={() => setAuthState("unlocked")}
        onSubmit={setupMasterPassword}
      />
    );
  }

  if (authState === "locked") {
    return (
      <AuthPage
        mode="unlock"
        onSuccess={() => setAuthState("unlocked")}
        onSubmit={(password) => unlockApp(password)}
        onReset={() => setAuthState("setup")}
        openResetModal={menuResetTriggered}
        onResetModalOpened={() => setMenuResetTriggered(false)}
      />
    );
  }

  return (
    <BrowserRouter>
      <MenuEventHandler />
      <div className="flex h-screen w-screen overflow-hidden bg-gray-950">
        <Sidebar />
        <main className="flex-1 overflow-y-auto">
          <Routes>
            <Route path="/" element={<RepositoriesPage />} />
            <Route path="/snapshots/:repoId" element={<SnapshotsPage />} />
            <Route path="/snapshots/:repoId/:snapshotId/browse" element={<BrowsePage />} />
            <Route path="/backup-plans" element={<BackupPlansPage />} />
            <Route path="/backup-plans/:planId" element={<BackupPlanEditPage />} />
            <Route path="/schedules" element={<SchedulesPage />} />
            <Route path="/schedules/:scheduleId" element={<ScheduleEditPage />} />
            <Route path="/logs" element={<LogsPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}
