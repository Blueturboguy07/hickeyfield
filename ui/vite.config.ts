import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

/**
 * Strip Vite's `crossorigin` attribute from emitted script/link tags.
 *
 * Tauri serves built assets over the `tauri://` custom protocol, which does not
 * emit CORS headers. A crossorigin module fetch against an opaque-origin
 * protocol is blocked outright — and the symptom is a completely blank white
 * window with nothing useful in the console. This is not optional.
 */
function stripCrossorigin(): Plugin {
  return {
    name: "strip-crossorigin",
    enforce: "post",
    transformIndexHtml(html) {
      return html.replace(/\s+crossorigin/g, "");
    },
  };
}

export default defineConfig({
  plugins: [react(), stripCrossorigin()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // The Rust side has its own rebuild loop; watching it just burns CPU.
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  build: {
    // Match the oldest WKWebView we support. Tauri uses the system webview, so
    // this is a real floor, not a nicety.
    target: "safari15",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
  test: {
    environment: "jsdom",
    globals: true,
  },
});
