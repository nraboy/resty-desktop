import type { DisplayState } from "../lib/planTree";
import { CheckIcon } from "./icons";

export const STATE_LABEL: Record<DisplayState, string> = {
  included: "Included",
  partial: "Partly included",
  excluded: "Excluded",
  contains: "Contains a backup source",
  unverified: "Included (not fully checked)",
  pending: "Checking",
};

/**
 * The checkbox-style marker. Filled = backed up in full, dash = backed up but something inside is
 * left out, empty = not backed up. A folder that is not backed up itself but holds a source has
 * its own look (dashed outline, dot) so it can't be mistaken for "partly included".
 */
export function StateBox({ state }: { state: DisplayState }) {
  const base = "inline-flex items-center justify-center w-4 h-4 rounded-[3px] border flex-shrink-0";
  switch (state) {
    case "included":
      return (
        <span className={`${base} bg-blue-600 border-blue-600 text-white`} aria-hidden="true">
          <CheckIcon className="w-3 h-3" />
        </span>
      );
    case "unverified":
      return (
        <span className={`${base} bg-blue-600/70 border-blue-500 border-dashed text-white`} aria-hidden="true">
          <CheckIcon className="w-3 h-3" />
        </span>
      );
    case "partial":
      return (
        <span className={`${base} border-blue-500 bg-blue-900/40`} aria-hidden="true">
          <span className="block w-2 h-[2px] bg-blue-400 rounded" />
        </span>
      );
    case "contains":
      return (
        <span className={`${base} border-blue-500 border-dashed`} aria-hidden="true">
          <span className="block w-1.5 h-1.5 rounded-full bg-blue-400" />
        </span>
      );
    case "pending":
      return <span className={`${base} border-gray-600 animate-pulse`} aria-hidden="true" />;
    default:
      return <span className={`${base} border-gray-600`} aria-hidden="true" />;
  }
}

/** Marks a row that is one of the plan's backup sources (the folders and files the tree builds). */
export function SourcePill() {
  return (
    <span className="flex-shrink-0 text-[10px] font-medium uppercase tracking-wide px-1.5 py-px rounded border border-blue-700/50 bg-blue-900/40 text-blue-300">
      Source
    </span>
  );
}
