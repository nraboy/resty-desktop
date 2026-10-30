import { describe, it, expect } from "vitest";
import { activeSectionIndex } from "./scrollSpy";

// Three sections whose tops sit at 0, 300 and 700 px in a tall pane (not scrolled to the bottom).
const TOPS = [0, 300, 700];
const at = (scrollTop: number) => activeSectionIndex(TOPS, scrollTop, 400, 2000);

describe("activeSectionIndex", () => {
  it("is the first section at the top", () => {
    expect(at(0)).toBe(0);
  });

  it("is the last section whose top is within the slack (40 px) of the scroll position", () => {
    expect(at(259)).toBe(0);
    expect(at(260)).toBe(1);
    expect(at(270)).toBe(1);
    expect(at(299)).toBe(1);
    expect(at(659)).toBe(1);
    expect(at(660)).toBe(2);
    expect(at(1500)).toBe(2);
  });

  it("is the last section once the pane is scrolled to the bottom, wherever its top is", () => {
    expect(activeSectionIndex(TOPS, 500, 400, 900)).toBe(2);
    // Within 2 px of the bottom counts (fractional scroll positions).
    expect(activeSectionIndex(TOPS, 498, 400, 900)).toBe(2);
    expect(activeSectionIndex(TOPS, 497, 400, 900)).toBe(1);
  });

  it("honours a custom slack", () => {
    expect(activeSectionIndex(TOPS, 290, 400, 2000, 0)).toBe(0);
    expect(activeSectionIndex(TOPS, 300, 400, 2000, 0)).toBe(1);
  });

  it("is 0 with no sections, or above the first one", () => {
    expect(activeSectionIndex([], 0, 400, 2000)).toBe(0);
    expect(activeSectionIndex([100, 300], 0, 400, 2000)).toBe(0);
  });
});
