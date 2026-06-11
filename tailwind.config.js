/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        blue: {
          300: "rgb(var(--tw-blue-300) / <alpha-value>)",
          400: "rgb(var(--tw-blue-400) / <alpha-value>)",
          700: "rgb(var(--tw-blue-700) / <alpha-value>)",
          900: "rgb(var(--tw-blue-900) / <alpha-value>)",
        },
        green: {
          400: "rgb(var(--tw-green-400) / <alpha-value>)",
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
