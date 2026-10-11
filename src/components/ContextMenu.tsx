import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";

// Items are deliberately label-only: no menu item in the app carries an icon. The
// iconed surface is the row-level `ActionButton`, where the glyph does real work
// (its label only reveals as text at the `wide` breakpoint, so below that the icon
// is the whole affordance). In a menu the text is always present, so a glyph would
// be decoration — and one iconed item among many also indents its own label past
// every other, which reads as a layout bug. See docs/frontend.md.
export type ContextMenuItemDef =
  | {
      label: string;
      onClick: () => void;
      variant?: "default" | "danger";
      disabled?: boolean;
      separator?: never;
    }
  | { separator: true; label?: never; onClick?: never; variant?: never; disabled?: never };

type Props = {
  x: number;
  y: number;
  items: ContextMenuItemDef[];
  onClose: () => void;
};

export default function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onMouseDown = () => onClose();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("mousedown", onMouseDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onMouseDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  // nudge menu onto screen if it would overflow
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    if (rect.right > window.innerWidth) el.style.left = `${x - rect.width}px`;
    if (rect.bottom > window.innerHeight) el.style.top = `${y - rect.height}px`;
  }, [x, y]);

  return createPortal(
    <div
      ref={ref}
      style={{ position: "fixed", left: x, top: y, zIndex: 9999 }}
      className="min-w-[168px] bg-gray-900 border border-gray-700 rounded-lg shadow-xl py-1 text-sm"
      onMouseDown={(e) => e.stopPropagation()}
    >
      {items.map((item, i) =>
        "separator" in item && item.separator ? (
          <div key={i} className="my-1 border-t border-gray-800" />
        ) : (
          <button
            key={i}
            disabled={item.disabled}
            onClick={() => { onClose(); item.onClick?.(); }}
            className={`w-full text-left px-3 py-1.5 transition-colors disabled:opacity-40 disabled:cursor-not-allowed ${
              item.variant === "danger"
                ? "text-red-400 hover:bg-red-900/30 hover:text-red-300"
                : "text-gray-300 hover:bg-gray-800 hover:text-gray-50"
            }`}
          >
            {item.label}
          </button>
        )
      )}
    </div>,
    document.body
  );
}
