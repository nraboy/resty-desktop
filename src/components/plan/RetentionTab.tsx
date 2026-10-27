import RetentionFields from "../RetentionFields";
import { hasRetention } from "../../lib/planForm";
import { CARD, type FormProps } from "./types";

/** `seededFromDefault`: a new plan whose fields were pre-filled from Settings' default policy. */
export default function RetentionTab({ form, update, seededFromDefault = false }: FormProps & { seededFromDefault?: boolean }) {
  return (
    <div className={CARD}>
      <h2 className="text-sm font-medium text-gray-300 mb-1">Retention Policy (optional)</h2>
      <p className="text-xs text-gray-500 mb-4">
        After each backup, snapshots outside this policy are forgotten (and pruned, unless the repository defers pruning). Leave all fields blank to skip retention.
      </p>
      <RetentionFields value={form.retention} onChange={(retention) => update({ retention })} />
      {seededFromDefault && hasRetention(form) && (
        <p className="mt-3 text-xs text-gray-500">
          Pre-filled from your default retention policy in Settings. Clear the fields to skip retention.
        </p>
      )}
    </div>
  );
}
