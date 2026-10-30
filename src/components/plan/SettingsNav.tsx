import { SETTINGS_SECTIONS, SETTINGS_SECTION_LABELS, type SettingsSection } from "../../lib/planForm";

interface Props {
  /** The section being read (scroll-spy), marked with the accent bar and `aria-current`. */
  active: SettingsSection;
  /** Whether a section is set away from its default (a small blue dot). */
  custom: (s: SettingsSection) => boolean;
  /** The section where the last save failed (a red dot), if any. */
  alert: SettingsSection | null;
  onJump: (s: SettingsSection) => void;
}

/**
 * The Settings tab's quiet jump list: section names only, a thin accent bar on the one being
 * read, a blue dot for a section set away from its default, a red dot where the last save failed.
 */
export default function SettingsNav({ active, custom, alert, onJump }: Props) {
  return (
    <nav aria-label="Settings sections" className="w-48 flex-shrink-0 pt-5 pl-6 pr-3">
      <ul className="border-l border-gray-800">
        {SETTINGS_SECTIONS.map((s) => {
          const current = s === active;
          return (
            <li key={s}>
              <button
                type="button"
                aria-current={current ? "true" : undefined}
                onClick={() => onJump(s)}
                className={`relative flex w-full items-center gap-2 py-1.5 pl-3 pr-1 text-left text-sm rounded-r-md transition-colors focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 ${
                  current ? "text-gray-50 font-medium" : "text-gray-400 hover:text-gray-200"
                }`}
              >
                {current && <span aria-hidden="true" className="absolute -left-px inset-y-1 w-0.5 rounded-full bg-blue-500" />}
                <span className="truncate">{SETTINGS_SECTION_LABELS[s]}</span>
                {alert === s ? (
                  <span className="w-1.5 h-1.5 flex-shrink-0 rounded-full bg-red-400" role="img" aria-label="needs attention" />
                ) : custom(s) ? (
                  <span className="w-1.5 h-1.5 flex-shrink-0 rounded-full bg-blue-400" role="img" aria-label="has custom settings" />
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
