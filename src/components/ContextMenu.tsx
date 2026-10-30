import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { nextEnabled } from "../lib/menuNav";

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
  // One slot per entry in `items`; separators have no button, so theirs stays null.
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  // What had focus when the menu opened, so Escape/Tab can hand focus back to it.
  const opener = useRef<HTMLElement | null>(null);

  // The disabled state lives on the buttons themselves, so keyboard navigation reads it from
  // there. Separators (no button) and disabled items are both `false`, so nextEnabled skips them.
  const enabledFlags = () => Array.from(itemRefs.current, (el) => !!el && !el.disabled);

  // Mount only (not on every parent re-render — callers pass an inline `onClose`, and re-running
  // this would pull focus back to the first item mid-use): remember where focus was, then move it
  // into the menu so the arrow keys work straight away.
  useEffect(() => {
    const active = document.activeElement;
    // StrictMode runs this twice in dev; by the second run focus is already inside the menu, and
    // that must not overwrite the element it came from.
    if (!ref.current?.contains(active)) opener.current = active instanceof HTMLElement ? active : null;
    const first = nextEnabled("Home", -1, enabledFlags());
    if (first !== null) itemRefs.current[first]?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    const onMouseDown = () => onClose();
    // Escape (and Tab, below) close the menu and give focus back to where it came from. Choosing
    // an item deliberately does not: its action may open a dialog that must keep the focus it takes.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        opener.current?.focus();
      }
    };
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

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Tab") {
      // The items are tabIndex -1, so Tab would otherwise walk off into the page behind the
      // still-open menu; close it like a native menu does.
      e.preventDefault();
      onClose();
      opener.current?.focus();
      return;
    }
    const current = itemRefs.current.findIndex((el) => el !== null && el === document.activeElement);
    const next = nextEnabled(e.key, current, enabledFlags());
    if (next === null) return; // Enter/Space activate the focused button natively
    e.preventDefault();
    itemRefs.current[next]?.focus();
  };

  return createPortal(
    <div
      ref={ref}
      role="menu"
      style={{ position: "fixed", left: x, top: y, zIndex: 9999 }}
      className="min-w-[168px] bg-gray-900 border border-gray-700 rounded-lg shadow-xl py-1 text-sm"
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={onKeyDown}
    >
      {items.map((item, i) =>
        "separator" in item && item.separator ? (
          <div key={i} role="separator" className="my-1 border-t border-gray-800" />
        ) : (
          <button
            key={i}
            ref={(el) => { itemRefs.current[i] = el; }}
            role="menuitem"
            tabIndex={-1}
            disabled={item.disabled}
            onClick={() => { onClose(); item.onClick?.(); }}
            // The focus ring is inset so it stays inside the menu's rounded border; keyboard focus
            // gets the hover look too. focus-visible (not focus) keeps a right-click-opened menu
            // looking exactly as it did before.
            className={`w-full text-left px-3 py-1.5 transition-colors disabled:opacity-40 disabled:cursor-not-allowed focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-blue-500 ${
              item.variant === "danger"
                ? "text-red-400 hover:bg-red-900/30 hover:text-red-300 focus-visible:bg-red-900/30 focus-visible:text-red-300"
                : "text-gray-300 hover:bg-gray-800 hover:text-gray-50 focus-visible:bg-gray-800 focus-visible:text-gray-50"
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
