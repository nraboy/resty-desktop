# Activity panel → overlay drawer

## Context

The Activity panel (`src/components/ActivityPanel.tsx`) is currently an **in-flow flex
sibling** of the routed page content. In the app shell (`src/App.tsx:156-196`) three flex-row
siblings share the width: `Sidebar` (`w-56`, fixed), the center content column (`flex-1`), and
`ActivityPanel` (right). The panel renders a 24px rail (`w-6`) when collapsed and a 256px drawer
(`w-64`) when expanded — both `flex-shrink-0` — so opening it squeezes the center `flex-1`
column by 232px and **reflows every routed page** (Snapshots table, Logs, Browse tree, etc.).
There is no animation; it swaps one fixed-width element for another.

The panel is a *transient glance surface* — you open it to check a running backup / index, then
dismiss it. Reflowing the content you were reading just to peek at progress is jarring. The goal
is to make the expanded panel an **overlay on top of the content** so nothing reflows, while
preserving the ambient "something is running" signal.

**Agreed design (from user):**
- Keep the slim 24px rail as the always-visible trigger + ambient active-dot indicator.
- **No scrim** — the app behind stays fully visible/legible (follow `ContextMenu`, not `Modal`).
- Dismiss via **click-outside** and the **chevron button**. (No Escape key — deliberately out.)
- Add a slide-in/out transition, now that we're free of the layout-swap constraint.

## Approach

Restructure `ActivityPanel.tsx` so the rail stays in flow (reserving its 24px, so content width
never changes) and the expanded drawer becomes a `fixed` overlay that slides in over the content.

### 1. Rail stays an in-flow flex sibling (unchanged footprint)

Always render the existing collapsed rail button (`ActivityPanel.tsx:56-68`) as the flex child —
`flex-shrink-0 w-6 …`, blue active-dot when `hasActive`, `onClick={() => setOpen(true)}`. Because
it always occupies its 24px slot, opening/closing the overlay causes **zero layout shift** in the
center column (its width matches today's collapsed state).

### 2. Expanded drawer becomes a fixed, always-mounted overlay

Convert the expanded `<aside>` (`ActivityPanel.tsx:71-149`) from `w-64 flex-shrink-0 …` to a
fixed, right-anchored overlay that is **always mounted** but slid off-screen when closed (so the
open/close transition animates both ways). Keep all inner content (Active/Upcoming/Recent
sections) verbatim.

- Container classes: `fixed inset-y-0 right-0 w-64 z-40 bg-gray-900 border-l border-gray-800
  flex flex-col overflow-y-auto shadow-xl transition-transform duration-200`
  plus `translate-x-0` when `open`, else `translate-x-full pointer-events-none`.
- `z-40` sits **below** `Modal.tsx`'s `z-50` so any progress modal still layers above the panel.
- `pointer-events-none` when closed prevents the off-screen drawer from intercepting clicks.
- No scrim element — do **not** add `Modal`'s `bg-black/60 backdrop-blur` backdrop.
- Chevron close button (`ActivityPanel.tsx:75-83`) keeps `onClick={() => setOpen(false)}`.

The rail and the overlay are now rendered together (rail in flow, overlay fixed on top) rather
than either/or via the current `if (!open) return …` early return (`ActivityPanel.tsx:54`).

### 3. Click-outside to dismiss

Add a `useEffect` that, **only while `open`**, attaches a `document` `mousedown` listener and
calls `setOpen(false)` when the event target is outside the drawer. Reuse the exact pattern from
`ContextMenu.tsx:24-33` (attach on open, remove on cleanup), minus the `keydown`/Escape branch.

- Add a `panelRef` (`useRef<HTMLElement>`) on the overlay `<aside>`; close when
  `!panelRef.current?.contains(e.target as Node)`.
- The rail button lives outside the drawer, but a click on it happens *before* `open` flips true,
  so the listener isn't attached yet — no immediate re-close race. (When open, the drawer covers
  the rail anyway.) No rail ref needed.

### Reused patterns
- Click-outside `useEffect`: `src/components/ContextMenu.tsx:24-33`.
- `fixed` overlay + z-index layering reference: `src/components/Modal.tsx:14-18` (`z-50`, so the
  panel must stay below it) — but **without** its scrim.
- All activity data (`indexing, activeBackup, upcoming, recentLogs`) still comes from
  `useActivity()` (`src/lib/activity.tsx`); no data-layer changes.

### Out of scope / unchanged
- `src/App.tsx` mount point (`App.tsx:194`) stays as-is — `<ActivityPanel />` remains the third
  flex sibling; only its internal rendering changes.
- `ActivityProvider`, the `useActivity` hook, and all sub-sections' markup are untouched.
- No Escape-key handler (per decision). No open-state persistence (still closed on launch).

## Verification

1. `npm run tauri dev`.
2. **No reflow:** open a page with a wide table (Snapshots or Logs). Click the rail to open the
   panel — confirm the table does **not** resize/reflow; the panel slides in over the right edge.
3. **Ambient signal:** trigger a backup or auto-index; with the panel closed, confirm the blue
   dot shows on the rail.
4. **Dismiss:** open the panel, then (a) click anywhere in the page content → closes; (b) reopen
   and click the chevron → closes. Both animate out.
5. **Layering:** start a manual backup/restore/prune (which opens its own `Modal`) with the
   panel open → confirm the modal renders above the panel, not behind it.
6. **No layout jump:** toggle open/closed repeatedly and confirm the center content's left/right
   position never shifts.
7. `npm run test:vite` (no test changes expected; confirm nothing regressed).
