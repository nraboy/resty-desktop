import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
/// <reference types="vitest" />

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  test: {
    environment: "node",
    // format.ts's date formatters (formatDate/formatDateOnly) construct a module-level
    // Intl.DateTimeFormat with no explicit timeZone or locale, so their output depends on
    // whatever timezone AND locale the process resolves at import time. Pinning only TZ here
    // and asserting exact strings in format.test.ts is not enough — verified by reproduction:
    // running the suite under LANG=de_DE.UTF-8 changed the formatted output and failed 3
    // assertions even with TZ pinned. LC_ALL/LANG must be pinned too. Test-only — this has no
    // effect on the built app, which always resolves the user's real timezone/locale.
    env: {
      TZ: "UTC",
      LANG: "en-US",
      LC_ALL: "en-US",
    },
  },
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
