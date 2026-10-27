import { supportsAppendOnly } from "../lib/backends";
import { InfoIcon } from "./icons";
import Tooltip from "./Tooltip";

type Props = {
  path: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
};

/**
 * "Append-only server" checkbox for a `rest:` repository. Renders nothing for other paths.
 * Ticking it disables everything that deletes or rewrites existing snapshots for this repo
 * (see repo.rs's ensure_can_delete). Test Connection also sets it automatically by asking the
 * server (detectAppendOnly), so this is mostly an override and a way to see the state.
 */
export default function AppendOnlyField({ path, checked, onChange }: Props) {
  if (!supportsAppendOnly(path)) return null;
  return (
    <div className="flex items-center gap-2 text-sm text-gray-400">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
        Append-only server (rest-server --append-only)
      </label>
      <Tooltip
        title="Append-only mode"
        content={
          <div className="space-y-2">
            <p>
              A rest-server started with <span className="font-mono">--append-only</span> accepts
              new backups but refuses to delete or modify existing data. That protects your
              backups if the machine being backed up is compromised.
            </p>
            <p>
              When this is on, deleting or tagging snapshots, retention and prune are disabled for
              this repository, because the server would reject them (a tag change would even leave
              a duplicate snapshot behind). Backups, restores, browsing and copying into the
              repository still work.
            </p>
            <p>Test Connection detects this automatically.</p>
          </div>
        }
      >
        <span
          tabIndex={0}
          role="img"
          aria-label="About append-only servers"
          className="cursor-help text-gray-500 hover:text-gray-300"
        >
          <InfoIcon className="w-4 h-4" />
        </span>
      </Tooltip>
    </div>
  );
}
