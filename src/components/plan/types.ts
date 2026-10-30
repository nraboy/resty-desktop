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

/** The small numeric inputs of the Bandwidth and Restic options sections. */
export const NUMBER_INPUT =
  "w-20 bg-gray-800 border border-gray-700 rounded-lg px-2 py-1.5 text-sm text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500";
