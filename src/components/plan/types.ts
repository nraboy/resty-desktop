import type { PlanFormState } from "../../lib/planForm";

/** Merge a patch into the plan form; a function patch sees the latest state. */
export type FormUpdate = (
  patch: Partial<PlanFormState> | ((prev: PlanFormState) => Partial<PlanFormState>),
) => void;

export interface FormProps {
  form: PlanFormState;
  update: FormUpdate;
}

/** Card chrome shared by every section of the editor. */
export const CARD = "bg-gray-900 border border-gray-800 rounded-xl p-4 mb-4";
