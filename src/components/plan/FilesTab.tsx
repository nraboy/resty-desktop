import { useEffect, useState } from "react";
import { treeSupported } from "../../lib/invoke";
import { treeRulesFromForm, type PlanFormState } from "../../lib/planForm";
import { summarizeEdit, type EditOutcome } from "../../lib/planTreeEdit";
import PlanTreePreview from "../PlanTreePreview";
import ExcludeCard from "./ExcludeCard";
import FdaNotice from "./FdaNotice";
import ListFilesCard from "./ListFilesCard";
import MarkersCard from "./MarkersCard";
import SourcesCard from "./SourcesCard";
import type { FormProps } from "./types";

/** The part of the form the tree edits. */
type Edited = Pick<PlanFormState, "paths" | "excludeItems" | "excludeText">;
const editedOf = (f: PlanFormState): Edited => ({ paths: f.paths, excludeItems: f.excludeItems, excludeText: f.excludeText });

/** One change made from the tree, with the state either side so it can be taken back. */
interface EditRecord {
  summary: string;
  before: Edited;
  after: Edited;
}

/** How many tree edits can be undone. */
const HISTORY_LIMIT = 20;

/** What gets backed up: the file tree (which is also where sources are picked), then the rules
 * that leave things out. */
export default function FilesTab({ form, update, active }: FormProps & { active: boolean }) {
  const [supported, setSupported] = useState<boolean | null>(null);
  useEffect(() => {
    treeSupported().then(setSupported).catch(() => setSupported(false));
  }, []);

  // Edits made from the tree, oldest first. Undo steps back through them.
  const [history, setHistory] = useState<EditRecord[]>([]);

  // Applies an edit confirmed in the tree: the verified new sources and patterns replace the
  // editor's, and the previous state is kept so the change can be undone.
  const applyEdit = (o: EditOutcome) => {
    const before = editedOf(form);
    const patch: Partial<Edited> = {
      paths: o.sources,
      // Expert mode keeps the user's text (comments, blank lines) — the outcome preserves it.
      ...(form.excludeMode === "expert" ? { excludeText: o.excludes.join("\n") } : { excludeItems: o.excludes }),
    };
    update(patch);
    setHistory((h) => [...h.slice(-(HISTORY_LIMIT - 1)), { summary: summarizeEdit(o), before, after: { ...before, ...patch } }]);
  };

  const undo = () => {
    const last = history[history.length - 1];
    if (!last) return;
    update(last.before);
    setHistory((h) => h.slice(0, -1));
  };

  // Once the sources or patterns are changed any other way (typing a pattern, picking a folder in
  // the fallback list), the recorded states no longer describe what's on screen; undoing them
  // would throw those changes away, so the history starts over.
  const editedKey = JSON.stringify(editedOf(form));
  useEffect(() => {
    setHistory((h) => (h.length > 0 && JSON.stringify(h[h.length - 1].after) !== editedKey ? [] : h));
  }, [editedKey]);

  return (
    <>
      {supported === false ? (
        <SourcesCard form={form} update={update} />
      ) : supported ? (
        <PlanTreePreview
          rules={treeRulesFromForm(form)}
          active={active}
          onApplyEdit={applyEdit}
          history={history}
          onUndo={undo}
        />
      ) : null}
      <ListFilesCard form={form} update={update} />
      <FdaNotice form={form} />
      <ExcludeCard form={form} update={update} />
      <MarkersCard form={form} update={update} />
    </>
  );
}
