import { useRef, type KeyboardEvent } from "react";
import { PLAN_TABS, PLAN_TAB_LABELS, type PlanTab } from "../../lib/planForm";

export interface TabInfo {
  /** One short line under the label ("Keep 7 daily", "2 tags"). */
  summary?: string;
  /** Something on the tab is set away from its default. */
  dot?: boolean;
  /** The last save found a problem on this tab. */
  alert?: boolean;
}

interface Props {
  active: PlanTab;
  onChange: (tab: PlanTab) => void;
  info: Partial<Record<PlanTab, TabInfo>>;
}

export const tabId = (t: PlanTab) => `plan-tab-${t}`;
export const panelId = (t: PlanTab) => `plan-panel-${t}`;

/**
 * The plan editor's tab strip: a roving-tabindex tablist (arrow keys, Home/End) with a short
 * summary under each label, an info dot for non-default settings, and a red dot on the tab where
 * the last save failed.
 */
export default function PlanTabs({ active, onChange, info }: Props) {
  const refs = useRef<Partial<Record<PlanTab, HTMLButtonElement | null>>>({});

  const onKey = (e: KeyboardEvent<HTMLButtonElement>, tab: PlanTab) => {
    const i = PLAN_TABS.indexOf(tab);
    let next: PlanTab | null = null;
    if (e.key === "ArrowRight") next = PLAN_TABS[(i + 1) % PLAN_TABS.length];
    else if (e.key === "ArrowLeft") next = PLAN_TABS[(i - 1 + PLAN_TABS.length) % PLAN_TABS.length];
    else if (e.key === "Home") next = PLAN_TABS[0];
    else if (e.key === "End") next = PLAN_TABS[PLAN_TABS.length - 1];
    if (next) {
      e.preventDefault();
      onChange(next);
      refs.current[next]?.focus();
    }
  };

  return (
    <div role="tablist" aria-label="Backup plan sections" className="flex gap-1 border-b border-gray-800 mb-4">
      {PLAN_TABS.map((tab) => {
        const selected = tab === active;
        const i = info[tab];
        return (
          <button
            key={tab}
            ref={(el) => {
              refs.current[tab] = el;
            }}
            id={tabId(tab)}
            role="tab"
            type="button"
            aria-selected={selected}
            aria-controls={panelId(tab)}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(tab)}
            onKeyDown={(e) => onKey(e, tab)}
            className={`relative flex flex-col items-start justify-start px-4 py-2 text-left -mb-px border-b-2 transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 rounded-t-md ${
              selected
                ? "border-blue-500 text-gray-50"
                : "border-transparent text-gray-400 hover:text-gray-200 hover:border-gray-600"
            }`}
          >
            <span className="flex items-center gap-1.5 text-sm font-medium">
              {PLAN_TAB_LABELS[tab]}
              {i?.alert ? (
                <span className="w-1.5 h-1.5 rounded-full bg-red-400" role="img" aria-label="needs attention" />
              ) : i?.dot ? (
                <span className="w-1.5 h-1.5 rounded-full bg-blue-400" role="img" aria-label="has custom settings" />
              ) : null}
            </span>
            {i?.summary && <span className="block text-xs text-gray-500 mt-0.5 truncate max-w-[10rem]">{i.summary}</span>}
          </button>
        );
      })}
    </div>
  );
}
