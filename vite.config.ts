import { realpathSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { defineConfig, searchForWorkspaceRoot } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"

/**
 * Nessa UI is consumed as source, not as its published bundle.
 *
 * `tsup` bundles the whole package into a single `dist/index.js`, which hoists
 * every dependency to one module's top level — so `mermaid` and `katex` are
 * *static* imports of that file. Rollup then cannot drop them by tree-shaking
 * the components that use them, and an app using six components shipped
 * mermaid, cytoscape, and the whole KaTeX font set: ~6MB of chunks nothing
 * reaches. Against the source, every component is its own module and the ones
 * this app never imports are shaken out.
 *
 * `realpathSync` matters: the dependency is a symlink into a local checkout,
 * and Vite treats a resolved real path as project source to transform rather
 * than as a prebundled dependency.
 */
const nessaUiPkg = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "node_modules/@nessa-ui/react",
)
let nessaUi
try {
  nessaUi = resolve(realpathSync(nessaUiPkg), "src")
} catch {
  throw new Error(
    `Cannot resolve @nessa-ui/react at ${nessaUiPkg}. Run pnpm install — it clones nessalabs/nessa_ui into .vendor.`,
  )
}

// Tauri drives this dev server, so the port is fixed and the Rust sources are
// left to cargo's own watcher.
const host = process.env.TAURI_DEV_HOST

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
    // The design system is a symlink that often lives outside this checkout
    // (a sibling worktree, or a clone for a machine that does not have one).
    // Vite's default allow-list is the workspace root, so the realpath has
    // to be named or every component 404s in `pnpm app`.
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), dirname(nessaUi)] },
  },
  resolve: {
    alias: [
      { find: /^@nessa-ui\/react\//, replacement: `${nessaUi}/components/` },
      // The package's own internal alias. Scoped to the two prefixes it
      // actually uses rather than a bare `@`, which would also capture any
      // `@/…` this app later writes for itself.
      { find: /^@\/components\//, replacement: `${nessaUi}/components/` },
      { find: /^@\/lib\//, replacement: `${nessaUi}/lib/` },
    ],
    // The linked checkout carries its own React in devDependencies. Without
    // deduping, the app and the library each load a copy and every hook in the
    // library throws.
    dedupe: ["react", "react-dom"],
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    // The webview is WKWebView on macOS and WebKit2GTK elsewhere; both are
    // comfortably past this baseline, and Tauri sets the env vars in CI builds.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
    rollupOptions: {
      treeshake: {
        // The package's own package.json declares that only its stylesheets
        // have side effects and its modules do not, but that field is not
        // consulted for source reached through an alias. Restating it lets
        // Rollup drop a component this app never imports along with the
        // stylesheet that component pulls in — which is what was still
        // shipping the whole KaTeX font set on MathBlock's behalf.
        moduleSideEffects: (id: string) =>
          !id.startsWith(nessaUi) || id.endsWith(".css"),
      },
    },
  },
})
