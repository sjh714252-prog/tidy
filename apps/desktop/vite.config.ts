import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev port and no screen clearing (see tauri.conf.json).
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Never watch the Rust build output: on Windows, Vite trying to watch the
    // freshly linked tidy_desktop_lib.dll fails with EBUSY and kills dev mode.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // WebView2 (Windows) is Chromium; WKWebView on macOS 13+.
    target: ["es2022", "chrome105", "safari15"],
    sourcemap: false,
  },
});
