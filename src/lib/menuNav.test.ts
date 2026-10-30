import { describe, it, expect } from "vitest";
import { nextEnabled } from "./menuNav";

describe("nextEnabled", () => {
  const flags = [true, false, true];

  it("ArrowDown moves to the next enabled item, skipping a disabled one", () => {
    expect(nextEnabled("ArrowDown", 0, flags)).toBe(2);
  });

  it("ArrowDown wraps from the last item to the first", () => {
    expect(nextEnabled("ArrowDown", 2, flags)).toBe(0);
  });

  it("ArrowUp wraps from the first item to the last", () => {
    expect(nextEnabled("ArrowUp", 0, flags)).toBe(2);
  });

  it("ArrowUp moves to the previous enabled item, skipping a disabled one", () => {
    expect(nextEnabled("ArrowUp", 2, flags)).toBe(0);
  });

  it("Home goes to the first enabled item and End to the last", () => {
    expect(nextEnabled("Home", 2, flags)).toBe(0);
    expect(nextEnabled("End", 0, flags)).toBe(2);
  });

  it("Home and End skip disabled items at the ends", () => {
    const edges = [false, true, true, false];
    expect(nextEnabled("Home", 2, edges)).toBe(1);
    expect(nextEnabled("End", 1, edges)).toBe(2);
  });

  it("treats separators (false entries) like disabled items", () => {
    // [item, separator, item, separator, item]
    const withSeparators = [true, false, true, false, true];
    expect(nextEnabled("ArrowDown", 0, withSeparators)).toBe(2);
    expect(nextEnabled("ArrowDown", 2, withSeparators)).toBe(4);
    expect(nextEnabled("ArrowUp", 4, withSeparators)).toBe(2);
  });

  it("with nothing focused yet, ArrowDown picks the first and ArrowUp the last", () => {
    expect(nextEnabled("ArrowDown", -1, flags)).toBe(0);
    expect(nextEnabled("ArrowUp", -1, flags)).toBe(2);
    expect(nextEnabled("ArrowDown", -1, [false, true, true])).toBe(1);
  });

  it("stays put when the current item is the only enabled one", () => {
    const one = [false, true, false];
    expect(nextEnabled("ArrowDown", 1, one)).toBe(1);
    expect(nextEnabled("ArrowUp", 1, one)).toBe(1);
  });

  it("returns null for keys it does not handle", () => {
    expect(nextEnabled("a", 0, flags)).toBeNull();
    expect(nextEnabled("Enter", 0, flags)).toBeNull();
    expect(nextEnabled(" ", 0, flags)).toBeNull();
    expect(nextEnabled("Escape", 0, flags)).toBeNull();
    expect(nextEnabled("Tab", 0, flags)).toBeNull();
  });

  it("returns null when no item is enabled", () => {
    const none = [false, false, false];
    for (const key of ["ArrowDown", "ArrowUp", "Home", "End"]) {
      expect(nextEnabled(key, 0, none)).toBeNull();
    }
    expect(nextEnabled("ArrowDown", 0, [])).toBeNull();
  });
});
