// Narrow, high-signal lint config: react-hooks rules only.
//
// Deliberately does NOT enable typescript-eslint's rule sets or any stylistic
// rules — tsc --noEmit already covers type errors, and stylistic linting adds
// churn without preventing the regressions this project actually sees. The
// typescript-eslint parser is used only so ESLint can parse .tsx/.ts syntax
// (JSX, type annotations, etc.) — see CLAUDE.md's Testing section.
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default tseslint.config(
  {
    ignores: ["dist/**", "src-tauri/**", "node_modules/**"],
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
      globals: {
        ...globals.browser,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
    },
  },
);
