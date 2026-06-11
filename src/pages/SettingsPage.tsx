import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-shell";
import { changeMasterPassword, clearBrowseCache, getCompression, getResticPath, getResticVersion, setCompression as saveCompression, setResticPath } from "../lib/invoke";
import { useTheme } from "../lib/theme";
import type { Theme } from "../lib/theme";
import Button from "../components/Button";
import Input from "../components/Input";

const THEMES: { value: Theme; label: string; description: string }[] = [
  { value: "system", label: "System", description: "Follow the OS appearance" },
  { value: "light",  label: "Light",  description: "Always use the light theme" },
  { value: "dark",   label: "Dark",   description: "Always use the dark theme" },
];

export default function SettingsPage() {
  const { theme, setTheme } = useTheme();
  const [resticPath, setResticPathLocal] = useState("restic");
  const [compression, setCompression] = useState("auto");
  const [resticVersion, setResticVersion] = useState<string | null>(null);
  const [versionError, setVersionError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  const [clearingCache, setClearingCache] = useState(false);
  const [cacheCleared, setCacheCleared] = useState(false);

  const [oldPassword, setOldPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [changingPassword, setChangingPassword] = useState(false);
  const [passwordChanged, setPasswordChanged] = useState(false);
  const [passwordError, setPasswordError] = useState("");

  useEffect(() => {
    getResticPath().then(setResticPathLocal).catch(() => {});
    getCompression().then(setCompression).catch(() => {});
    getResticVersion()
      .then((v) => { setResticVersion(v); setVersionError(""); })
      .catch((e) => { setResticVersion(null); setVersionError(String(e)); });
  }, []);

  const handleSave = async () => {
    setSaving(true);
    setError("");
    setSaved(false);
    try {
      await setResticPath(resticPath);
      await saveCompression(compression);
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      getResticVersion()
        .then((v) => { setResticVersion(v); setVersionError(""); })
        .catch((e) => { setResticVersion(null); setVersionError(String(e)); });
    } catch (err: any) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleClearCache = async () => {
    setClearingCache(true);
    try {
      await clearBrowseCache();
      setCacheCleared(true);
      setTimeout(() => setCacheCleared(false), 2000);
    } finally {
      setClearingCache(false);
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
      setTimeout(() => setPasswordChanged(false), 3000);
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
          {resticVersion && (
            <p className="mt-2 text-xs text-green-400 font-mono">{resticVersion}</p>
          )}
          {versionError && (
            <p className="mt-2 text-xs text-red-400">{versionError}</p>
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
        {error && <p className="text-sm text-red-400">{error}</p>}
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

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
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
          {passwordError && <p className="text-sm text-red-400">{passwordError}</p>}
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

      <div className="mt-6 bg-gray-900 border border-gray-800 rounded-xl p-5">
        <h2 className="text-sm font-medium text-gray-300 mb-1">Browse Cache</h2>
        <p className="text-xs text-gray-500 mb-3">
          Snapshot file listings are cached locally to speed up navigation. Clear the cache if you
          see stale data or want to free up disk space.
        </p>
        <div className="flex items-center gap-3">
          <Button variant="secondary" onClick={handleClearCache} loading={clearingCache}>
            Clear Browse Cache
          </Button>
          {cacheCleared && (
            <span className="text-sm text-green-400 flex items-center gap-1">
              <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
              Cleared
            </span>
          )}
        </div>
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
