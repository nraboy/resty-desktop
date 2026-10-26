/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      screens: {
        // Reveal text labels beside row-action icons above this width. The app's window
        // minimum is tauri.conf.json's minWidth (1024), so stock `xl` (1280) toggles too and
        // marks the compact tier: compact styles are written only as `max-xl:` (active below
        // 1280) or `xl:hidden` (compact-only elements), purely additive — nothing at >=1280
        // is ever edited for them. `wide` keeps its own job below; the other stock breakpoints
        // are unused. Both are in logical (CSS) px, same units as minWidth; see
        // docs/frontend.md for the units caveat (a physical 1920px monitor is often well under
        // 1920 logical px). `wide` is deliberately well above typical MacBook logical widths
        // (13" Air ~1470, 14" Pro ~1512) so labels only reveal on a genuinely wide window — a
        // maximized external display, not a laptop's own screen — minimizing the chance of row
        // content wrapping.
        // Mid-point of the compact tier (1024-1279): the icon rail is already active here, so at
        // 1152 the content area (~1040px) is wider than the full layout's at 1280 (~1008px) —
        // enough to bring back one lower-value column (Snapshots Host, Logs Repository). Used
        // only as `max-mid:` (active below 1152), same additive rule as `max-xl:`.
        mid: "1152px",
        wide: "1536px",
      },
      colors: {
        blue: {
          300: "rgb(var(--tw-blue-300) / <alpha-value>)",
          400: "rgb(var(--tw-blue-400) / <alpha-value>)",
          700: "rgb(var(--tw-blue-700) / <alpha-value>)",
          900: "rgb(var(--tw-blue-900) / <alpha-value>)",
        },
        green: {
          300: "rgb(var(--tw-green-300) / <alpha-value>)",
          400: "rgb(var(--tw-green-400) / <alpha-value>)",
          700: "rgb(var(--tw-green-700) / <alpha-value>)",
          900: "rgb(var(--tw-green-900) / <alpha-value>)",
        },
        red: {
          300: "rgb(var(--tw-red-300) / <alpha-value>)",
          400: "rgb(var(--tw-red-400) / <alpha-value>)",
          700: "rgb(var(--tw-red-700) / <alpha-value>)",
          900: "rgb(var(--tw-red-900) / <alpha-value>)",
        },
        amber: {
          300: "rgb(var(--tw-amber-300) / <alpha-value>)",
          400: "rgb(var(--tw-amber-400) / <alpha-value>)",
          500: "rgb(var(--tw-amber-500) / <alpha-value>)",
          700: "rgb(var(--tw-amber-700) / <alpha-value>)",
          900: "rgb(var(--tw-amber-900) / <alpha-value>)",
        },
        purple: {
          400: "rgb(var(--tw-purple-400) / <alpha-value>)",
        },
        yellow: {
          400: "rgb(var(--tw-yellow-400) / <alpha-value>)",
        },
        gray: {
          50:  "rgb(var(--tw-gray-50)  / <alpha-value>)",
          100: "rgb(var(--tw-gray-100) / <alpha-value>)",
          200: "rgb(var(--tw-gray-200) / <alpha-value>)",
          300: "rgb(var(--tw-gray-300) / <alpha-value>)",
          400: "rgb(var(--tw-gray-400) / <alpha-value>)",
          500: "rgb(var(--tw-gray-500) / <alpha-value>)",
          600: "rgb(var(--tw-gray-600) / <alpha-value>)",
          700: "rgb(var(--tw-gray-700) / <alpha-value>)",
          800: "rgb(var(--tw-gray-800) / <alpha-value>)",
          900: "rgb(var(--tw-gray-900) / <alpha-value>)",
          950: "rgb(var(--tw-gray-950) / <alpha-value>)",
        },
      },
    },
  },
  plugins: [],
};
