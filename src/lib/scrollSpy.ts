// Scroll-spy for a pane of stacked sections (the plan editor's Settings tab): which section the
// jump list highlights for a scroll position. Pure, so the boundaries are unit-tested
// (scrollSpy.test.ts); the component only measures and calls this.

/**
 * The index of the section being read: the last one whose top has reached the top of the pane
 * (within `slack` px, so a section counts a little before its heading touches the edge). Once the
 * pane is scrolled to the bottom it's the last section, even if its top can't reach — otherwise a
 * short last section could never be highlighted. `0` with no sections or above the first one.
 *
 * `tops` are each section's offset from the top of the pane's content, in order.
 */
export function activeSectionIndex(
  tops: number[],
  scrollTop: number,
  viewportHeight: number,
  scrollHeight: number,
  slack = 40,
): number {
  if (tops.length === 0) return 0;
  // Within 2 px: scroll positions can be fractional.
  if (scrollTop + viewportHeight >= scrollHeight - 2) return tops.length - 1;
  let active = 0;
  tops.forEach((top, i) => {
    if (top - slack <= scrollTop) active = i;
  });
  return active;
}
