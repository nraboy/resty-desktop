interface ProgressBarProps {
  /** 0–100, clamped. Ignored when `indeterminate` is true. */
  percent?: number;
  /** Renders a constantly-sliding bar instead of a percent-width fill — for work with no
   *  measurable progress (e.g. a single-repo prune, which reports no item counts). */
  indeterminate?: boolean;
  /** Tailwind background color class for the fill. */
  colorClass?: string;
  /** Tailwind height class, shared by the track and the fill. */
  heightClass?: string;
  /** Extra classes on the outer track div (e.g. spacing like "mb-4"). */
  className?: string;
}

/** Shared progress bar for modals and the Activity panel — determinate (a real `percent`) or
 *  indeterminate (a looping slide, driven by the `slide` keyframe in index.css) for operations
 *  that report no incremental progress. */
export default function ProgressBar({
  percent,
  indeterminate = false,
  colorClass = "bg-blue-500",
  heightClass = "h-1.5",
  className = "",
}: ProgressBarProps) {
  return (
    <div className={`w-full bg-gray-800 rounded-full ${heightClass} overflow-hidden ${className}`}>
      {indeterminate ? (
        <div className={`${heightClass} w-1/3 rounded-full ${colorClass} animate-[slide_1.4s_ease-in-out_infinite]`} />
      ) : (
        <div
          className={`${colorClass} ${heightClass} rounded-full transition-all duration-500`}
          style={{ width: `${Math.min(100, Math.max(0, percent ?? 0)).toFixed(1)}%` }}
        />
      )}
    </div>
  );
}
