import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { treeExport, treeExportWrite } from "../lib/invoke";
import type { TreeRulesInput } from "../lib/planTree";
import type { ExportOut } from "../lib/planTreeEdit";
import Button from "./Button";
import Modal from "./Modal";

interface Props {
  open: boolean;
  rules: TreeRulesInput;
  onClose: () => void;
}

/**
 * Shows how to reproduce this plan's include/exclude behaviour with the plain restic CLI: the
 * exclude file(s) and the `restic backup` command line, rendered by the same rule set the GUI
 * backs up with. The repository and password are never written out.
 */
export default function PlanTreeExportDialog({ open, rules, onClose }: Props) {
  const [data, setData] = useState<ExportOut | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState<string[] | null>(null);

  useEffect(() => {
    if (!open) return;
    setData(null);
    setError(null);
    setSaved(null);
    setCopied(false);
    treeExport(rules)
      .then(setData)
      .catch((e) => setError(String(e)));
    // The rules are captured when the dialog opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const copy = async () => {
    if (!data) return;
    try {
      await navigator.clipboard.writeText(data.command);
      setCopied(true);
    } catch {
      setError("Couldn't copy to the clipboard — select the command and copy it by hand.");
    }
  };

  const save = async () => {
    if (!data) return;
    const dir = await openDialog({ directory: true, multiple: false, title: "Save the exclude files in…" });
    if (typeof dir !== "string") return;
    try {
      setSaved(await treeExportWrite(dir, data.excludeFileName, data.excludeFile, data.iexcludeFileName, data.iexcludeFile));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Modal title="Use this plan with the restic CLI" open={open} onClose={onClose} maxWidth="max-w-2xl">
      <div className="space-y-4">
        {error && <p className="text-sm text-amber-400 whitespace-pre-wrap break-words">{error}</p>}
        {!data && !error && <p className="text-sm text-gray-500">Preparing…</p>}
        {data && (
          <>
            <p className="text-sm text-gray-400">
              Save the file{data.iexcludeFile ? "s" : ""} below next to where you run the command; restic then includes exactly what
              this plan does.
            </p>
            <div>
              <div className="flex items-center justify-between mb-1">
                <span className="text-xs text-gray-500">Command</span>
                <button type="button" onClick={copy} className="text-xs text-gray-400 hover:text-gray-50">
                  {copied ? "Copied" : "Copy"}
                </button>
              </div>
              <pre className="text-xs font-mono text-gray-300 bg-gray-800 border border-gray-700 rounded-lg p-3 whitespace-pre-wrap break-all select-text">
                {data.command}
              </pre>
            </div>
            <details>
              <summary className="text-xs text-gray-400 cursor-default">{data.excludeFileName}</summary>
              <pre className="mt-1 text-xs font-mono text-gray-300 bg-gray-800 border border-gray-700 rounded-lg p-3 max-h-56 overflow-auto select-text">
                {data.excludeFile}
              </pre>
            </details>
            {data.iexcludeFile && data.iexcludeFileName && (
              <details>
                <summary className="text-xs text-gray-400 cursor-default">{data.iexcludeFileName}</summary>
                <pre className="mt-1 text-xs font-mono text-gray-300 bg-gray-800 border border-gray-700 rounded-lg p-3 max-h-56 overflow-auto select-text">
                  {data.iexcludeFile}
                </pre>
              </details>
            )}
            <ul className="space-y-1">
              {data.notes.map((n, i) => (
                <li key={i} className="text-xs text-gray-500">{n}</li>
              ))}
            </ul>
            {saved && (
              <p className="text-xs text-green-300 break-all">Saved: {saved.join(", ")}</p>
            )}
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={onClose}>Close</Button>
              <Button onClick={save}>Save files…</Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  );
}
