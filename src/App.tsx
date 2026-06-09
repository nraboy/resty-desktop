import { BrowserRouter, Routes, Route } from "react-router-dom";
import Sidebar from "./components/Sidebar";
import RepositoriesPage from "./pages/RepositoriesPage";
import SnapshotsPage from "./pages/SnapshotsPage";
import BrowsePage from "./pages/BrowsePage";
import BackupPlansPage from "./pages/BackupPlansPage";
import BackupPlanEditPage from "./pages/BackupPlanEditPage";
import SettingsPage from "./pages/SettingsPage";

export default function App() {
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
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}
