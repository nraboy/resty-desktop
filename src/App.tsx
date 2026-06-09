import { useEffect, useState } from "react";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import Sidebar from "./components/Sidebar";
import RepositoriesPage from "./pages/RepositoriesPage";
import SnapshotsPage from "./pages/SnapshotsPage";
import BrowsePage from "./pages/BrowsePage";
import BackupPlansPage from "./pages/BackupPlansPage";
import BackupPlanEditPage from "./pages/BackupPlanEditPage";
import SettingsPage from "./pages/SettingsPage";
import LogsPage from "./pages/LogsPage";
import AuthPage from "./pages/AuthPage";
import { isAppSetup, setupMasterPassword, unlockApp } from "./lib/invoke";

type AuthState = "loading" | "setup" | "locked" | "unlocked";

export default function App() {
  const [authState, setAuthState] = useState<AuthState>("loading");

  useEffect(() => {
    isAppSetup()
      .then((setup) => setAuthState(setup ? "locked" : "setup"))
      .catch(() => setAuthState("setup"));
  }, []);

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
      />
    );
  }

  return (
    <BrowserRouter>
      <div className="flex h-screen w-screen overflow-hidden bg-gray-950">
        <Sidebar />
        <main className="flex-1 overflow-y-auto">
          <Routes>
            <Route path="/" element={<RepositoriesPage />} />
            <Route path="/snapshots/:repoId" element={<SnapshotsPage />} />
            <Route path="/snapshots/:repoId/:snapshotId/browse" element={<BrowsePage />} />
            <Route path="/backup-plans" element={<BackupPlansPage />} />
            <Route path="/backup-plans/:planId" element={<BackupPlanEditPage />} />
            <Route path="/logs" element={<LogsPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}
