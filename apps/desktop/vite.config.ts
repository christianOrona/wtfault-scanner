import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The UI is a pure client of the localhost API documented in docs/API.md.
// In `npm run dev` it talks to a server you started yourself:
//   cargo run -p aim-api -- --simulator --scenario dpf-regen
// Requests go through this proxy so the browser has no CORS story to get wrong;
// under Tauri the same paths are fetched directly from the bundled server.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // Never watch the Rust side. `cargo build` rewrites executables under
    // src-tauri/target while the dev server is running, and the watcher dies on
    // Windows with EBUSY when it tries to follow one - taking the UI down for a
    // reason that has nothing to do with the UI.
    watch: { ignored: ["**/src-tauri/**"] },
    proxy: {
      "/api": {
        target: process.env.AIM_API_URL ?? "http://127.0.0.1:8787",
        changeOrigin: true,
        ws: true,
      },
    },
  },
  build: { outDir: "dist", target: "es2021", sourcemap: true },
});
