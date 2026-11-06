import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Set by the Tauri CLI when the dev server must be reachable from another device.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  // Keep Rust errors visible in the terminal.
  clearScreen: false,
  server: {
    // Tauri expects a fixed port; fail instead of picking another one.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // The Rust side is rebuilt by the Tauri CLI, not by Vite.
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    // macOS 15 ships Safari 18's WebKit; this target is safely below it.
    target: "safari16",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
