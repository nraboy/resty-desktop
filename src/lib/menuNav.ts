// Keyboard navigation for ContextMenu, kept pure so it can be unit-tested without a DOM.

/**
 * Where focus should move for a keypress in a menu, or `null` if the key is not a navigation key
 * (or nothing in the menu can take focus).
 *
 * `enabled[i]` is true for a focusable item; separators and disabled items are `false` and are
 * skipped. `current` is the index of the focused item, or -1 when none is focused yet — then
 * ArrowDown picks the first enabled item and ArrowUp the last. Arrow keys wrap around.
 */
export function nextEnabled(key: string, current: number, enabled: boolean[]): number | null {
  const first = enabled.indexOf(true);
  if (first === -1) return null;
  const last = enabled.lastIndexOf(true);

  switch (key) {
    case "Home":
      return first;
    case "End":
      return last;
    case "ArrowDown":
    case "ArrowUp": {
      if (current < 0 || current >= enabled.length) return key === "ArrowDown" ? first : last;
      const step = key === "ArrowDown" ? 1 : -1;
      const n = enabled.length;
      // Walk at most one full lap; landing back on `current` means it is the only enabled item.
      for (let k = 1; k <= n; k++) {
        const i = (((current + step * k) % n) + n) % n;
        if (enabled[i]) return i;
      }
      return null;
    }
    default:
      return null;
  }
}
