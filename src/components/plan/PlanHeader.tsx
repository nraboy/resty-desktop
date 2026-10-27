import { useNavigate } from "react-router-dom";
import type { Repository } from "../../lib/types";
import Button from "../Button";
import { ChevronDownIcon, PencilIcon } from "../icons";

interface Props {
  name: string;
  onName: (name: string) => void;
  /** The name (or repository) was the problem on the last save. */
  nameInvalid: boolean;
  repoInvalid: boolean;
  repos: Repository[];
  repoId: string;
  onRepo: (id: string) => void;
  isNew: boolean;
  saving: boolean;
  onSave: () => void;
  onCancel: () => void;
  onDelete: () => void;
}

/**
 * The plan's title (an editable h1), its repository, and the Save / Cancel / Delete actions.
 * Sticky, so the actions are within reach on every tab and an error on a tab that is scrolled
 * out of view can't strand the user.
 */
export default function PlanHeader({
  name,
  onName,
  nameInvalid,
  repoInvalid,
  repos,
  repoId,
  onRepo,
  isNew,
  saving,
  onSave,
  onCancel,
  onDelete,
}: Props) {
  const navigate = useNavigate();
  const selected = repos.find((r) => r.id === repoId);
  return (
    <div className="sticky top-0 z-20 bg-gray-950 px-6 pt-5 pb-3 border-b border-gray-800/70">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <h1 className="group relative max-w-xl">
            <input
              id="plan-name"
              aria-label="Plan name"
              aria-invalid={nameInvalid}
              value={name}
              onChange={(e) => onName(e.target.value)}
              placeholder="Untitled plan"
              spellCheck={false}
              autoComplete="off"
              className={`w-full bg-transparent text-xl font-semibold text-gray-100 placeholder-gray-500 pr-7 py-0.5 border-b focus:outline-none focus:border-blue-500 ${
                nameInvalid ? "border-red-500" : "border-transparent hover:border-gray-700"
              }`}
            />
            <span className="pointer-events-none absolute right-1 top-1.5 text-gray-600 group-hover:text-gray-400 group-focus-within:text-gray-400">
              <PencilIcon className="w-4 h-4" />
            </span>
          </h1>

          <div className="mt-2 flex items-center gap-2 max-w-xl">
            <label htmlFor="plan-repo" className="text-xs font-medium text-gray-500 uppercase tracking-wider flex-shrink-0">
              Repository
            </label>
            {repos.length === 0 ? (
              <p className="text-sm text-gray-500">
                None configured.{" "}
                <button className="text-blue-400 hover:underline" onClick={() => navigate("/")}>
                  Add one first.
                </button>
              </p>
            ) : (
              <div className="relative flex-1">
                <select
                  id="plan-repo"
                  value={repoId}
                  aria-invalid={repoInvalid}
                  onChange={(e) => onRepo(e.target.value)}
                  className={`w-full appearance-none bg-gray-800 border rounded-md px-3 py-1.5 text-sm text-gray-100 focus:outline-none focus:ring-2 focus:ring-blue-500 pr-8 ${
                    repoInvalid ? "border-red-500" : "border-gray-700"
                  }`}
                >
                  <option value="" disabled>Select a repository…</option>
                  {repos
                    // A backup plan can never target a read-only repo — except the plan's
                    // already-selected repo, kept visible-but-disabled so an existing plan whose
                    // repo has since become read-only isn't silently dropped.
                    .filter((r) => !r.readOnly || r.id === repoId)
                    .map((r) => (
                      <option key={r.id} value={r.id} disabled={r.readOnly}>
                        {r.name} — {r.path}{r.readOnly ? " (read-only)" : ""}
                      </option>
                    ))}
                </select>
                <div className="pointer-events-none absolute inset-y-0 right-2 flex items-center text-gray-500">
                  <ChevronDownIcon className="w-4 h-4" />
                </div>
              </div>
            )}
          </div>
          {selected?.appendOnly && (
            <p className="text-sm text-amber-400 mt-2">
              This repository is on an append-only server, so retention rules can't be applied to it —
              the server refuses to delete snapshots. Backups still run; old snapshots are kept.
            </p>
          )}
          {selected?.readOnly && (
            <p className="text-sm text-amber-400 mt-2">
              This plan's repository is marked read-only — backups can't run until you either
              select a different repository or clear the read-only flag on Repositories.
            </p>
          )}
        </div>

        <div className="flex items-center gap-2 flex-shrink-0 pt-0.5">
          {!isNew && (
            <Button variant="danger" onClick={onDelete} disabled={saving}>
              Delete
            </Button>
          )}
          <Button variant="secondary" onClick={onCancel} disabled={saving}>
            Cancel
          </Button>
          <Button onClick={onSave} loading={saving}>
            {isNew ? "Create Plan" : "Save Changes"}
          </Button>
        </div>
      </div>
    </div>
  );
}
