import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

/**
 * WCAG AA contrast regression test for the theme palette in src/index.css.
 *
 * Guards against a palette edit silently making bare text unreadable — the same
 * class of bug as amber-400 rendering invisible on a white background in light
 * mode (see CLAUDE.md "Theming"). Text-capable shades must hold >= 4.5:1
 * against every surface they sit on:
 *   - dark mode: bg-gray-950 (page), bg-gray-900 (cards/modals), bg-gray-800 (inputs/tabs)
 *   - light mode: bg-gray-950 (page), bg-gray-900 (panels)
 * The -700/-900 accent shades are excluded: they are badge backgrounds and
 * borders, never bare text.
 */

type Rgb = [number, number, number];

function relativeLuminance([r, g, b]: Rgb): number {
  const channel = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrastRatio(a: Rgb, b: Rgb): number {
  const [l1, l2] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
}

/** Extract the `--tw-*` declarations from one CSS selector block in index.css. */
function parseBlock(css: string, selector: string): Record<string, Rgb> {
  const start = css.indexOf(selector);
  expect(start, `selector ${selector} not found in index.css`).toBeGreaterThan(-1);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  const body = css.slice(open, close);
  const vars: Record<string, Rgb> = {};
  for (const match of body.matchAll(/--tw-([a-z]+-\d+):\s*(\d+)\s+(\d+)\s+(\d+)/g)) {
    vars[match[1]] = [Number(match[2]), Number(match[3]), Number(match[4])];
  }
  return vars;
}

// Vitest stubs CSS imports (even ?raw) to an empty string, so read the file directly.
const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "index.css"), "utf-8");
const dark = parseBlock(css, ":root");
const light = parseBlock(css, "html.light");
const system = parseBlock(css, "html.system");

/** Shades used as bare text on page/card backgrounds. */
const TEXT_SHADES = {
  gray: ["100", "200", "300", "400", "500"],
  blue: ["300", "400", "500"],
  green: ["300", "400", "500"],
  red: ["300", "400", "500"],
  amber: ["300", "400", "500"],
  purple: ["400"],
  yellow: ["400"],
};

function textColors(palette: Record<string, Rgb>): Array<[string, Rgb]> {
  const out: Array<[string, Rgb]> = [];
  for (const [family, shades] of Object.entries(TEXT_SHADES)) {
    for (const shade of shades) {
      const key = `${family}-${shade}`;
      if (palette[key]) out.push([key, palette[key]]);
    }
  }
  return out;
}

describe("dark mode text contrast (WCAG AA)", () => {
  const surfaces: Array<[string, Rgb]> = [
    ["bg-gray-950 (page)", dark["gray-950"]],
    ["bg-gray-900 (cards/modals)", dark["gray-900"]],
    ["bg-gray-800 (inputs/tabs)", dark["gray-800"]],
  ];

  it.each(textColors(dark))("%s passes 4.5:1 on every dark surface", (name, color) => {
    for (const [surface, bg] of surfaces) {
      const ratio = contrastRatio(color, bg);
      expect(ratio, `${name} on ${surface}: ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("light mode text contrast (WCAG AA)", () => {
  const surfaces: Array<[string, Rgb]> = [
    ["bg-gray-950 (page)", light["gray-950"]],
    ["bg-gray-900 (panels)", light["gray-900"]],
    ["bg-gray-800 (inputs/tabs)", light["gray-800"]],
  ];

  it.each(textColors(light))("%s passes 4.5:1 on every light surface", (name, color) => {
    for (const [surface, bg] of surfaces) {
      const ratio = contrastRatio(color, bg);
      expect(ratio, `${name} on ${surface}: ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
    }
  });
});

describe("system theme block", () => {
  // The `html.system` block (inside the prefers-color-scheme: light media query) must stay
  // byte-identical to `html.light` — a shade tuned in one but not the other is exactly how
  // amber-400 shipped invisible on white for System-theme users only.
  it("html.system is identical to html.light", () => {
    expect(system).toEqual(light);
  });
});
