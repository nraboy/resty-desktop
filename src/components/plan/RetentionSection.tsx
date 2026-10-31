import RetentionFields from "../RetentionFields";
import { hasRetention } from "../../lib/planForm";
import { CARD, type FormProps } from "./types";

/** The Settings tab's Retention section (its title is the section heading).
 * `seededFromDefault`: a new plan whose fields were pre-filled from Settings' default policy. */
export default function RetentionSection({ form, update, seededFromDefault = false }: FormProps & { seededFromDefault?: boolean }) {
  return (
    <div className={CARD}>
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
