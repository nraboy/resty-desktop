import { type ReactNode, cloneElement, isValidElement, useState } from "react";
import {
  autoUpdate,
  flip,
  FloatingPortal,
  offset,
  shift,
  useDismiss,
  useFloating,
  useFocus,
  useHover,
  useInteractions,
  useRole,
} from "@floating-ui/react";

type Props = {
  /** Optional heading rendered above `content`, separated by a rule — for a label naming
   *  what the list/figure below it is (e.g. "Paths"), not a repeat of the trigger's text. */
  title?: ReactNode;
  /** Tooltip body. Any React content — not just text — since these replace `title` for
   *  hovers that need real formatting (multi-line lists, monospace, etc). */
  content: ReactNode;
  children: ReactNode;
};

/**
 * Styled hover tooltip for *content* hovers — multi-line or formatted information that a
 * native `title` attribute can't render well (delay, no styling, no line breaks in some
 * browsers). Icon-button label tooltips should stay on native `title`; see docs/frontend.md
 * for the split rule.
 *
 * Styled as ContextMenu's sibling (same surface: bg-gray-900/border-gray-700/rounded-lg/
 * shadow-xl) so the two floating surfaces read as one system.
 *
 * Not hoverable-into (no safePolygon) and never scrollable — callers should cap long content
 * (e.g. "first 5, then +N more") rather than relying on this component to make an unbounded
 * list usable.
 */
export default function Tooltip({ title, content, children }: Props) {
  const [open, setOpen] = useState(false);
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: setOpen,
    placement: "top",
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip(), shift({ padding: 8 })],
  });

  const hover = useHover(context, { move: false, delay: { open: 300, close: 0 }, restMs: 100 });
  const focus = useFocus(context);
  const dismiss = useDismiss(context);
  const role = useRole(context, { role: "tooltip" });

  const { getReferenceProps, getFloatingProps } = useInteractions([hover, focus, dismiss, role]);

  if (!isValidElement(children)) return children;

  const trigger = cloneElement(
    children as React.ReactElement<Record<string, unknown>>,
    getReferenceProps({ ref: refs.setReference, ...(children.props as Record<string, unknown>) })
  );

  return (
    <>
      {trigger}
      {context.open && (
        <FloatingPortal>
          <div
            ref={refs.setFloating}
            style={{ ...floatingStyles, zIndex: 9999 }}
            className="max-w-sm break-words bg-gray-900 border border-gray-700 rounded-lg shadow-xl px-3 py-2 text-xs text-gray-300"
            {...getFloatingProps()}
          >
            {title && (
              <div className="font-medium text-gray-200 mb-1 pb-1 border-b border-gray-800">{title}</div>
            )}
            {content}
          </div>
        </FloatingPortal>
      )}
    </>
  );
}
