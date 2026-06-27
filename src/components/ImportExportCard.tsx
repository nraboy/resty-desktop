import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  exportData,
  importBackrestConfig,
  importData,
  listRepos,
  previewBackrestImport,
  previewImport,
} from "../lib/invoke";
import type { ExportSummary, ImportPreview } from "../lib/types";
import Button from "./Button";
import Input from "./Input";
import Modal from "./Modal";

function summaryText(s: ExportSummary | ImportPreview): string {
  return [
    `${s.repos} ${s.repos === 1 ? "repository" : "repositories"}`,
    `${s.plans} ${s.plans === 1 ? "backup plan" : "backup plans"}`,
    `${s.schedules} ${s.schedules === 1 ? "schedule" : "schedules"}`,
  ].join(", ");
}

export default function ImportExportCard() {
  const [searchParams, setSearchParams] = useSearchParams();

  // Whether the install has any repositories — drives the passphrase requirement.
  const [hasRepos, setHasRepos] = useState(false);

  // ── export state ──
  const [exportOpen, setExportOpen] = useState(false);
  const [exportPw, setExportPw] = useState("");
  const [exportPwConfirm, setExportPwConfirm] = useState("");
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState("");
  const [exportDone, setExportDone] = useState<ExportSummary | null>(null);

  // ── import state ──
  const [importOpen, setImportOpen] = useState(false);
  const [importSource, setImportSource] = useState<"resty" | "backrest">("resty");
  const [importFile, setImportFile] = useState("");
  const [importPw, setImportPw] = useState("");
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState("");
  const [importDone, setImportDone] = useState<ExportSummary | null>(null);

  const refreshHasRepos = () => {
    listRepos().then((r) => setHasRepos(r.length > 0)).catch(() => {});
  };

  useEffect(refreshHasRepos, []);

  useEffect(() => {
    const action = searchParams.get("action");
    if (action === "import" || action === "export") {
      setSearchParams({}, { replace: true });
      if (action === "import") openImport();
      else openExport();
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchParams]);

  const openExport = () => {
    setExportPw("");
    setExportPwConfirm("");
    setExportError("");
    setExportDone(null);
    setExporting(false);
    refreshHasRepos();
    setExportOpen(true);
  };

  const handleExport = async () => {
    setExportError("");
    if (hasRepos) {
      if (exportPw.length < 8) {
        setExportError("Export passphrase must be at least 8 characters.");
        return;
      }
      if (exportPw !== exportPwConfirm) {
        setExportError("Passphrases do not match.");
        return;
      }
    }
    const path = await saveDialog({
      defaultPath: "resty-export.json",
      filters: [{ name: "Resty Desktop Export", extensions: ["json"] }],
    });
    if (!path) return;
    setExporting(true);
    try {
      const summary = await exportData(path, hasRepos ? exportPw : undefined);
      setExportDone(summary);
    } catch (err: any) {
      setExportError(String(err));
    } finally {
      setExporting(false);
    }
  };

  const openImport = () => {
    setImportSource("resty");
    setImportFile("");
    setImportPw("");
    setPreview(null);
    setImportError("");
    setImportDone(null);
    setPreviewing(false);
    setImporting(false);
    setImportOpen(true);
  };

  // Switching source clears any file already chosen — the two formats are parsed
  // by different backends and a preview from one doesn't apply to the other.
  const switchImportSource = (source: "resty" | "backrest") => {
    if (source === importSource) return;
    setImportSource(source);
    setImportFile("");
    setImportPw("");
    setPreview(null);
    setImportError("");
  };

  const chooseImportFile = async () => {
    const file = await openDialog({
      multiple: false,
      filters: [{ name: importSource === "backrest" ? "Backrest Config" : "Resty Desktop Export", extensions: ["json"] }],
    });
    if (typeof file !== "string") return;
    setImportFile(file);
    setImportPw("");
    setImportError("");
    setPreviewing(true);
    try {
      const p = importSource === "backrest" ? await previewBackrestImport(file) : await previewImport(file);
      setPreview(p);
    } catch (err: any) {
      setImportError(String(err));
      setPreview(null);
    } finally {
      setPreviewing(false);
    }
  };

  const handleImport = async () => {
    setImportError("");
    if (preview?.requiresPassword && importPw.length === 0) {
      setImportError("Enter the export passphrase.");
      return;
    }
    setImporting(true);
    try {
      const summary =
        importSource === "backrest"
          ? await importBackrestConfig(importFile)
          : await importData(importFile, preview?.requiresPassword ? importPw : undefined);
      setImportDone(summary);
      refreshHasRepos();
    } catch (err: any) {
      setImportError(String(err));
    } finally {
      setImporting(false);
    }
  };

  return (
    <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
      <h2 className="text-sm font-medium text-gray-300 mb-1">Import &amp; Export</h2>
      <p className="text-xs text-gray-500 mb-3">
        Save a full snapshot of your repositories, backup plans, and schedules to a file, or restore
        one on another installation. Repository passwords are encrypted with an export passphrase you
        choose.
      </p>
      <div className="flex items-center gap-3">
        <Button variant="secondary" onClick={openExport}>Export…</Button>
        <Button variant="secondary" onClick={openImport}>Import…</Button>
      </div>

      {/* ── Export modal ── */}
      <Modal open={exportOpen} onClose={() => setExportOpen(false)} title="Export Data">
        {exportDone ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">Exported {summaryText(exportDone)}.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setExportOpen(false)}>Close</Button>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            <p className="text-sm text-gray-400">
              This exports everything — all repositories, backup plans, and schedules — to a single
              file.
            </p>
            {hasRepos ? (
              <div className="space-y-3">
                <p className="text-xs text-gray-500">
                  Choose an export passphrase to encrypt your repository passwords. You will need it
                  again to import.
                </p>
                <Input
                  type="password"
                  label="Export Passphrase"
                  placeholder="At least 8 characters"
                  value={exportPw}
                  onChange={(e) => setExportPw(e.target.value)}
                  onClear={() => setExportPw("")}
                />
                <Input
                  type="password"
                  label="Confirm Passphrase"
                  placeholder="Re-enter passphrase"
                  value={exportPwConfirm}
                  onChange={(e) => setExportPwConfirm(e.target.value)}
                  onClear={() => setExportPwConfirm("")}
                />
              </div>
            ) : (
              <p className="text-xs text-gray-500">
                No passphrase is needed because there are no repository passwords to protect.
              </p>
            )}
            {exportError && <p className="text-sm text-red-300">{exportError}</p>}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setExportOpen(false)}>Cancel</Button>
              <Button onClick={handleExport} loading={exporting}>Choose File &amp; Export</Button>
            </div>
          </div>
        )}
      </Modal>

      {/* ── Import modal ── */}
      <Modal open={importOpen} onClose={() => setImportOpen(false)} title="Import Data">
        {importDone ? (
          <div className="space-y-4">
            <p className="text-sm text-gray-300">Imported {summaryText(importDone)} as new copies.</p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={() => setImportOpen(false)}>Close</Button>
            </div>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="flex rounded-lg overflow-hidden border border-gray-700">
              <button
                type="button"
                onClick={() => switchImportSource("resty")}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${importSource === "resty" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Resty Desktop
              </button>
              <button
                type="button"
                onClick={() => switchImportSource("backrest")}
                className={`flex-1 py-1.5 text-sm font-medium transition-colors ${importSource === "backrest" ? "bg-gray-700 text-gray-100" : "bg-gray-800 text-gray-500 hover:text-gray-300"}`}
              >
                Backrest
              </button>
            </div>

            {importSource === "backrest" && (
              <p className="text-xs text-gray-500">
                Select a Backrest <span className="font-mono">config.json</span> to import its
                repositories, plans, and schedules into Resty Desktop.
              </p>
            )}

            <div className="flex gap-2">
              <div className="flex-1">
                <Input
                  value={importFile}
                  readOnly
                  placeholder="Select a file…"
                  className="w-full"
                  title={importFile}
                />
              </div>
              <Button variant="secondary" onClick={chooseImportFile} loading={previewing}>
                Browse
              </Button>
            </div>

            {preview && (
              <>
                <p className="text-sm text-gray-300">This file contains {summaryText(preview)}.</p>
                <div className="rounded-lg border border-amber-500/40 bg-gray-800 px-3 py-2">
                  <p className="text-xs font-medium text-amber-500 mb-1">Heads up</p>
                  <p className="text-xs text-gray-300">
                    Repository and backup paths are imported exactly as stored and may not exist on
                    this machine — review them after importing. Schedules are imported disabled so
                    backups don't fire until you re-enable them.
                  </p>
                  {importSource === "backrest" && (
                    <p className="text-xs text-gray-300 mt-1">
                      Backrest has features Resty Desktop doesn't, so not everything will carry over — the
                      core repositories, plans, and schedules above will be imported.
                    </p>
                  )}
                </div>
                <p className="text-xs text-gray-500">
                  Everything is imported as new copies with fresh identifiers; existing data is left
                  untouched.
                </p>
                {preview.requiresPassword && (
                  <Input
                    type="password"
                    label="Export Passphrase"
                    placeholder="Passphrase used during export"
                    value={importPw}
                    onChange={(e) => setImportPw(e.target.value)}
                    onClear={() => setImportPw("")}
                  />
                )}
              </>
            )}

            {importError && <p className="text-sm text-red-300">{importError}</p>}
            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={() => setImportOpen(false)}>Cancel</Button>
              <Button onClick={handleImport} loading={importing} disabled={!preview}>
                Confirm Import
              </Button>
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}
