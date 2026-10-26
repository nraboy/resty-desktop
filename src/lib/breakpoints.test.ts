import { describe, it, expect } from "vitest";
import defaultTheme from "tailwindcss/defaultTheme";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

/**
 * The icon-rail sidebar is driven by two things that must agree: Tailwind's `xl` breakpoint
 * (every `max-xl:`/`xl:hidden` class) and `useIsCompact` in Sidebar.tsx, which hardcodes the same
 * width in a `matchMedia` query to decide when nav links get a hover `title`. If someone overrides
 * `xl` in tailwind.config.js, or edits only one of the two numbers, labels would lose their
 * tooltips (or gain redundant ones) while the CSS layout stays put.
 */
const root = join(dirname(fileURLToPath(import.meta.url)), "../..");

describe("compact-tier breakpoint", () => {
  const xl = (defaultTheme.screens as Record<string, string>).xl;

  it("tailwind.config.js does not override the stock xl screen", async () => {
    const { default: config } = (await import(/* @vite-ignore */ join(root, "tailwind.config.js"))) as {
      default: { theme?: { screens?: Record<string, string>; extend?: { screens?: Record<string, string> } } };
    };
    // Replacing `theme.screens` would drop xl entirely; extending it with an `xl` key would move it.
    expect(config.theme?.screens).toBeUndefined();
    expect(config.theme?.extend?.screens?.xl).toBeUndefined();
    expect(xl).toBe("1280px");
  });

  it("Sidebar's useIsCompact query matches Tailwind's xl", () => {
    const sidebar = readFileSync(join(root, "src/components/Sidebar.tsx"), "utf-8");
    const m = /const query = "\(min-width: (\d+)px\)"/.exec(sidebar);
    expect(m).not.toBeNull();
    expect(`${m![1]}px`).toBe(xl);
  });
});
