import type { ButtonHTMLAttributes, ReactNode } from "react";

type Tone = "blue" | "green" | "purple" | "yellow" | "red";

interface ActionButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "title"> {
  icon: ReactNode;
  /** Concise action name — revealed as visible text at the `wide` breakpoint, and the
   * accessible name (title + aria-label) unless `title` overrides it. Keep this short:
   * it becomes inline button text, not just a tooltip. */
  label: string;
  /** Overrides the tooltip/aria-label without changing the visible `wide` text — use for a
   * longer disabled-state explanation ("This repository is read-only") that would look wrong
   * spelled out inline on the button itself. */
  title?: string;
  tone?: Tone;
}

const toneClasses: Record<Tone, string> = {
  blue: "hover:text-blue-400",
  green: "hover:text-green-400",
  purple: "hover:text-purple-400",
  yellow: "hover:text-yellow-400",
  red: "hover:text-red-300",
};

// Canonical row-action button: an icon that's always present, plus a text label that only
// renders as visible text once the window is wide enough (see tailwind.config.js's `wide`
// breakpoint) — a pure CSS reveal, no resize listener. The label is unconditionally the
// button's accessible name via title/aria-label, so keyboard and screen-reader users get it
// at every window width, not just wide ones. Replaces the two icon-only treatments this
// project used to have (bare `p-1.5` buttons in tables, `<Button variant="ghost" size="sm">`
// in card lists) with one shared shape.
export default function ActionButton({
  icon,
  label,
  title,
  tone = "blue",
  className = "",
  disabled,
  ...props
}: ActionButtonProps) {
  const accessibleName = title ?? label;
  return (
    <button
      {...props}
      disabled={disabled}
      title={accessibleName}
      aria-label={accessibleName}
      className={`inline-flex items-center gap-1.5 py-1.5 px-2.5 rounded-md text-gray-400 hover:bg-gray-800
        border border-transparent wide:border-gray-700 wide:hover:border-gray-600
        transition-colors disabled:opacity-50 disabled:cursor-not-allowed
        disabled:hover:text-gray-400 disabled:hover:bg-transparent disabled:wide:hover:border-gray-700
        ${toneClasses[tone]} ${className}`}
    >
      {icon}
      <span className="hidden wide:inline text-sm font-medium whitespace-nowrap">{label}</span>
    </button>
  );
}
