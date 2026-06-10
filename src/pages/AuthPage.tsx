import { useState, useEffect } from "react";
import Button from "../components/Button";
import Input from "../components/Input";
import Modal from "../components/Modal";
import { resetApp } from "../lib/invoke";

interface Props {
  mode: "setup" | "unlock";
  onSuccess: () => void;
  onSubmit: (password: string) => Promise<void>;
  onReset?: () => void;
  openResetModal?: boolean;
  onResetModalOpened?: () => void;
}

export default function AuthPage({ mode, onSuccess, onSubmit, onReset, openResetModal, onResetModalOpened }: Props) {
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  const [showResetModal, setShowResetModal] = useState(false);
  const [resetConfirm, setResetConfirm] = useState("");
  const [resetting, setResetting] = useState(false);
  const [resetError, setResetError] = useState("");

  const isSetup = mode === "setup";

  useEffect(() => {
    if (openResetModal) {
      openResetModalFn();
      onResetModalOpened?.();
    }
  }, [openResetModal]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");

    if (isSetup) {
      if (password.length < 8) {
        setError("Master password must be at least 8 characters.");
        return;
      }
      if (password !== confirm) {
        setError("Passwords do not match.");
        return;
      }
    }

    setLoading(true);
    try {
      await onSubmit(password);
      onSuccess();
    } catch (err: any) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleReset = async () => {
    if (resetConfirm !== "RESET") return;
    setResetting(true);
    setResetError("");
    try {
      await resetApp();
      setShowResetModal(false);
      onReset?.();
    } catch (err: any) {
      setResetError(String(err));
    } finally {
      setResetting(false);
    }
  };

  const openResetModalFn = () => {
    setResetConfirm("");
    setResetError("");
    setShowResetModal(true);
  };

  return (
    <div className="flex items-center justify-center h-screen w-screen bg-gray-950">
      <div className="w-full max-w-sm px-6">
        <div className="mb-8 text-center">
          <div className="flex justify-center mb-4">
            <svg className="w-10 h-10 text-blue-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
                d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z" />
            </svg>
          </div>
          <h1 className="text-xl font-semibold text-gray-100">
            {isSetup ? "Set Up Restic GUI" : "Welcome Back"}
          </h1>
          <p className="text-sm text-gray-500 mt-1">
            {isSetup
              ? "Create a master password to encrypt your repository credentials."
              : "Enter your master password to unlock."}
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <Input
            label="Master Password"
            type="password"
            placeholder="Enter master password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoFocus
          />
          {isSetup && (
            <Input
              label="Confirm Password"
              type="password"
              placeholder="Re-enter master password"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          )}

          {error && <p className="text-sm text-red-400">{error}</p>}

          <Button type="submit" loading={loading} className="w-full justify-center">
            {isSetup ? "Create & Unlock" : "Unlock"}
          </Button>
        </form>

        {!isSetup && (
          <div className="mt-5 text-center">
            <button
              className="text-xs text-gray-600 hover:text-gray-400 transition-colors"
              onClick={openResetModalFn}
            >
              Forgot your password?
            </button>
          </div>
        )}

        {isSetup && (
          <p className="mt-6 text-xs text-gray-600 text-center">
            Your master password encrypts all repository passwords using Argon2id + AES-256-GCM.
            It is never stored — if forgotten, the app must be reset.
          </p>
        )}
      </div>

      <Modal
        title="Reset Application"
        open={showResetModal}
        onClose={() => !resetting && setShowResetModal(false)}
      >
        <div className="space-y-4">
          <div className="p-3 bg-red-900/30 border border-red-700 rounded-lg">
            <p className="text-sm text-red-300 font-medium mb-1">This will permanently delete:</p>
            <ul className="text-xs text-red-400 space-y-0.5 list-disc list-inside">
              <li>All saved repositories</li>
              <li>All backup plans</li>
              <li>All app settings</li>
              <li>All cached snapshot data</li>
            </ul>
            <p className="text-xs text-red-400 mt-2">
              Your actual restic repositories and their data on disk are not affected.
            </p>
          </div>

          <p className="text-sm text-gray-300">
            Type <span className="font-mono font-semibold text-white">RESET</span> to confirm.
          </p>
          <Input
            placeholder="RESET"
            value={resetConfirm}
            onChange={(e) => setResetConfirm(e.target.value)}
            autoFocus
          />

          {resetError && <p className="text-sm text-red-400">{resetError}</p>}

          <div className="flex justify-end gap-2 pt-1">
            <Button
              variant="secondary"
              onClick={() => setShowResetModal(false)}
              disabled={resetting}
            >
              Cancel
            </Button>
            <Button
              variant="danger"
              loading={resetting}
              disabled={resetConfirm !== "RESET"}
              onClick={handleReset}
            >
              Reset App
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
