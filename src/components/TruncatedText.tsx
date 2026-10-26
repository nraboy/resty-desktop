import { useState } from "react";

type Props = {
  text: string;
  /** Callers supply the truncation classes (`truncate`, `max-xl:truncate`, …) — this component
   *  deliberately hard-codes none, so a cell that only truncates in the compact tier doesn't
   *  start doing so at full width. */
  className?: string;
  as?: "div" | "span" | "p";
};

/**
 * Text that may be cut off by CSS truncation, with a native `title` hover that appears only when
 * it actually is. The overflow check runs on mouseenter, so the title is absent for text that
 * fits (and everywhere at widths where the caller doesn't truncate). Native `title` rather than
 * `Tooltip`: this is one short string per cell, not formatted content, and it avoids a
 * floating-ui instance per table row — see docs/frontend.md for the split rule.
 */
export default function TruncatedText({ text, className, as: Tag = "div" }: Props) {
  // Whether the text overflowed at the last hover — kept as a flag, not the title string, so a
  // `text` that changes afterwards can never leave a stale title behind.
  const [truncated, setTruncated] = useState(false);
  return (
    <Tag
      className={className}
      title={truncated ? text : undefined}
      onMouseEnter={(e: React.MouseEvent<HTMLElement>) => {
        const el = e.currentTarget;
        setTruncated(el.scrollWidth > el.clientWidth);
      }}
    >
      {text}
    </Tag>
  );
}
