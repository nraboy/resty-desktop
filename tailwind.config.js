/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      screens: {
        // Reveal text labels beside row-action icons above this width. The app's window
        // never goes below tauri.conf.json's minWidth (1280), so stock sm/md/lg/xl are all
        // permanently true here — this is the one breakpoint that actually toggles. It's in
        // logical (CSS) px, same units as minWidth; see docs/frontend.md for the units
        // caveat (a physical 1920px monitor is often well under 1920 logical px). Kept close
        // to the 1280 minimum on purpose so the toggle is reachable on a laptop's built-in
        // display without maximizing — raise it if labels feel like they show too eagerly.
        wide: "1300px",
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
