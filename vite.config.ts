import { readFileSync, realpathSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { defineConfig, searchForWorkspaceRoot } from "vite"
import react from "@vitejs/plugin-react"
import tailwindcss from "@tailwindcss/vite"

import { gatewayOrigin, parseStage } from "./src/env/gateway-ports"
import { loadEnvironment } from "./src/env/environment"

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
// This dev server fronts the gateway of whichever stage this environment
// selects — `dev` when it says nothing, which listens beside the port an
// installed Nessa holds. One table decides that port: see src/env/gateway-ports.
// Proxying `dev` regardless would send the browser to a socket nothing is on
// as soon as the gateway beside it was started as anything else.
// Parsed, not cast. `as Stage` asserted a shape TypeScript could not check, so
// any string at all reached `gatewayOrigin` and a stage the server rejects
// resolved to a port here.
const stage = parseStage(process.env.NESSA_STAGE)
// `NESSA_PORT` as well as the stage, because the server reads both and this
// proxy is the browser's only way to it: overriding the port started a gateway
// on one socket and left every request going to the other, which answers as a
// gateway that is simply not there.
const override = (process.env.NESSA_PORT ?? "").trim()
const gatewayTarget =
  process.env.NESSA_BROWSER_GATEWAY_URL ??
  (override === "" ? gatewayOrigin(stage) : `http://127.0.0.1:${Number(override)}`)
const tlsCert = process.env.NESSA_BROWSER_TLS_CERT
const tlsKey = process.env.NESSA_BROWSER_TLS_KEY
if (Boolean(tlsCert) !== Boolean(tlsKey))
  throw new Error("Set both browser TLS certificate and key")

// A packaged frontend is built before Cargo embeds it. Leave one small record
// beside those assets so the host build can prove it is embedding UI for the
// same stage, including when `dist/` came from an earlier command.
let bundledStage: string | undefined

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    {
      name: "nessa-bundle-stage",
      apply: "build",
      configResolved(config) {
        // `config.env` is Vite's final environment after `.env.<mode>`, process
        // overrides, and DEV/PROD have been resolved. Passing those same values
        // through the UI's sole parser makes the record describe what
        // `import.meta.env` will make the application use.
        bundledStage = loadEnvironment(config.env, config.env.DEV).stage
      },
      generateBundle() {
        if (bundledStage === undefined)
          throw new Error("Vite did not resolve a bundle stage")
        this.emitFile({
          type: "asset",
          fileName: "nessa-stage.json",
          source: `${JSON.stringify({ stage: bundledStage })}\n`,
        })
      },
    },
  ],
  clearScreen: false,
  server: {
    https:
      tlsCert && tlsKey
        ? { cert: readFileSync(tlsCert), key: readFileSync(tlsKey) }
        : undefined,
    proxy: {
      "/browser": {
        target: gatewayTarget,
        ws: true,
      },
      // The gateway's pre-authentication surface, which setup asks before it
      // has a session. Proxied so a browser preview reaches it on its own
      // origin; the packaged app talks to the gateway directly.
      "/onboarding": {
        target: gatewayTarget,
      },
      // Where attachment bytes are uploaded. A browser preview derives the
      // upload origin from its proxied session, so the upload goes through here
      // too and stays same-origin; the packaged app uploads to the gateway
      // directly.
      "/attachments": {
        target: gatewayTarget,
      },
    },
    port: 1420,
    strictPort: true,
    // WebKitGTK resolves `localhost` to 127.0.0.1. Node's `true`/`false`
    // localhost bind is IPv6-only on this host, so a reload of the webview
    // gets connection-refused and the transparent window shows the desktop
    // with no chrome.
    host: host ?? "127.0.0.1",
    headers: {
      // WebKitGTK keeps module scripts after a restart; HMR then updates CSS
      // only, so a class added in JSX never lands on the node that is painted.
      "Cache-Control": "no-store",
    },
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      ignored: [
        "**/src-tauri/**",
        // The vendor clone is a whole monorepo. Watching Storybook and
        // validation tsconfigs forces a full reload on every install.
        "**/.vendor/nessa_ui/apps/**",
        "**/.vendor/nessa_ui/validation/**",
        // Agent worktrees are checkouts of this repo living inside it, and
        // something is usually building in one: generated client docs, a
        // `dist`, a test run. Each written file reloaded the page, so a panel
        // opened while another agent worked never finished painting — a blank
        // window, and no sign of why. What happens in another checkout is not
        // a change to this one.
        //
        // Anchored to this config's own directory rather than written as
        // `**/.claude/worktrees/**`: a worktree is itself a checkout, its path
        // contains that segment, and the loose pattern therefore matched the
        // source of whichever checkout was running — turning HMR off for
        // exactly the people who work in worktrees.
        `${resolve(dirname(fileURLToPath(import.meta.url)), ".claude/worktrees")}/**`,
      ],
    },
    // The design system is a symlink that often lives outside this checkout
    // (a sibling worktree, or a clone for a machine that does not have one).
    // Vite's default allow-list is the workspace root, so the realpath has
    // to be named or every component 404s in `pnpm app`.
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), dirname(nessaUi)] },
  },
  resolve: {
    alias: [
      { find: /^@nessa-ui\/react\//, replacement: `${nessaUi}/components/` },
      // The package's own internal alias. Scoped to the three prefixes it
      // actually uses rather than a bare `@`, which would also capture any
      // `@/…` this app later writes for itself.
      { find: /^@\/components\//, replacement: `${nessaUi}/components/` },
      { find: /^@\/lib\//, replacement: `${nessaUi}/lib/` },
      { find: /^@\/provider\//, replacement: `${nessaUi}/provider/` },
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
        moduleSideEffects: (id: string) => !id.startsWith(nessaUi) || id.endsWith(".css"),
      },
    },
  },
})
