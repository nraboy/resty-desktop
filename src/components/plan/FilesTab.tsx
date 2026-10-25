import { useState } from "react";
import { treeRulesFromForm } from "../../lib/planForm";
import { summarizeEdit, type EditOutcome } from "../../lib/planTreeEdit";
import PlanTreePreview from "../PlanTreePreview";
import ExcludeCard from "./ExcludeCard";
import MarkersCard from "./MarkersCard";
import SourcesCard from "./SourcesCard";
import type { FormProps } from "./types";

/** What gets backed up: the sources, a preview of the result, and the exclusion rules. */
export default function FilesTab({ form, update, active }: FormProps & { active: boolean }) {
  // The last include/exclude made from the preview, with a way to take it back.
  const [lastEdit, setLastEdit] = useState<{ summary: string; onUndo: () => void } | null>(null);

  // Applies an edit confirmed in the preview tree: the verified new sources and patterns replace
  // the editor's, and the previous state is kept so the change can be undone.
  const applyEdit = (o: EditOutcome) => {
    const before = { paths: form.paths, excludeItems: form.excludeItems, excludeText: form.excludeText };
    update({
      paths: o.sources,
      // Expert mode keeps the user's text (comments, blank lines) — the outcome preserves it.
      ...(form.excludeMode === "expert" ? { excludeText: o.excludes.join("\n") } : { excludeItems: o.excludes }),
    });
    setLastEdit({
      summary: summarizeEdit(o),
      onUndo: () => {
        update(before);
        setLastEdit(null);
      },
    });
  };

  return (
    <>
      <SourcesCard form={form} update={update} />
      <PlanTreePreview
        rules={treeRulesFromForm(form)}
        active={active}
        onApplyEdit={form.sourceMode === "paths" ? applyEdit : undefined}
        lastEdit={lastEdit}
        unavailableReason={
          form.sourceMode === "files" ? "The preview isn't available for plans that use path-list files yet." : undefined
        }
      />
      <ExcludeCard form={form} update={update} />
      <MarkersCard form={form} update={update} />
    </>
  );
}
