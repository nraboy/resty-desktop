import { useEffect, useState } from "react";
import { getResticPath, setResticPath } from "../lib/invoke";
import Button from "../components/Button";
import Input from "../components/Input";

export default function SettingsPage() {
  const [resticPath, setResticPathLocal] = useState("restic");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    getResticPath().then(setResticPathLocal).catch(() => {});
  }, []);

  const handleSave = async () => {
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await setResticPath(resticPath);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
    } catch (err: any) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="p-6">
      <div className="mb-6">
        <h1 className="text-xl font-semibold text-gray-100">Settings</h1>
        <p className="text-sm text-gray-500 mt-0.5">Configure Restic GUI behavior</p>
      </div>

      <div className="bg-gray-900 border border-gray-800 rounded-xl p-5 space-y-5">
        <div>
          <h2 className="text-sm font-medium text-gray-300 mb-1">Restic Binary Path</h2>
          <p className="text-xs text-gray-500 mb-3">
            Path to the <span className="font-mono">restic</span> executable. Defaults to{" "}
            <span className="font-mono text-gray-400">restic</span> (must be on{" "}
            <span className="font-mono text-gray-400">$PATH</span>).
          </p>
          <Input
            value={resticPath}
            onChange={(e) => setResticPathLocal(e.target.value)}
            placeholder="/usr/local/bin/restic"
          />
        </div>

        {error && (
          <p className="text-sm text-red-400">{error}</p>
        )}

        <div className="flex items-center gap-3">
          <Button onClick={handleSave} loading={saving}>
            Save Settings
          </Button>
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
        <h2 className="text-sm font-medium text-gray-300 mb-2">Install Restic</h2>
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
      </div>
    </div>
  );
}
