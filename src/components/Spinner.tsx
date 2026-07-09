interface SpinnerProps {
  /** Tailwind size + color classes, e.g. "w-6 h-6 text-blue-400". */
  className?: string;
}

/** Shared animated loading spinner — same markup previously duplicated inline across pages. */
export default function Spinner({ className = "w-5 h-5 text-blue-400" }: SpinnerProps) {
  return (
    <svg className={`animate-spin ${className}`} xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24">
      <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
      <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
    </svg>
  );
}
