// Vite config for the Sen Desktop frontend (bundler round, ZC Leader
// tin 9313: bundler is Vite, the Tauri 2 standard, lowest-risk choice).
// Frontend root is this directory (apps/desktop/): index.html lives here
// and the entry is /src/app/main.ts. The production output is dist/,
// which tauri.conf.json points at as frontendDist ("../dist", relative
// to src-tauri/). Dev server port 1420 matches devUrl in
// tauri.conf.json; strictPort keeps Tauri pointing at the right port
// instead of silently moving to the next free one.
// The frontend imports workspace packages (packages/core, ...) from
// outside this root, so the dev server is allowed to serve files from
// the whole workspace root (found via pnpm-workspace.yaml). No framework
// plugin: the UI shell is vanilla TypeScript (main.ts), no React/Vue.
// No model, no key, no network call is configured here.
import { searchForWorkspaceRoot } from "vite";
import { defineConfig } from "vite";

export default defineConfig({
  server: {
    port: 1420,
    strictPort: true,
    fs: {
      allow: [searchForWorkspaceRoot(process.cwd())],
    },
  },
  build: {
    outDir: "dist",
  },
});
