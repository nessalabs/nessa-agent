/**
 * `@nessa/client` as one ES module, for a dynamic app's server.
 *
 * The app host writes it as `node_modules/@nessa/client/index.mjs` beside
 * the app's own files, so `import { NessaClient } from "@nessa/client"` in
 * a server run by node or bun is Nessa's own client, of this build, without
 * a network install. `src-tauri/build.rs` runs this into its OUT_DIR.
 *
 *   node scripts/desktop/app-client-bundle.mjs <out-file>
 */
import { mkdirSync } from "node:fs"
import { basename, dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

import { build } from "vite"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")

export async function bundleAppClient(outFile) {
  mkdirSync(dirname(outFile), { recursive: true })
  await build({
    configFile: false,
    root,
    logLevel: "error",
    build: {
      lib: {
        entry: resolve(root, "packages/nessa-client/src/index.ts"),
        formats: ["es"],
        fileName: () => basename(outFile),
      },
      outDir: dirname(outFile),
      emptyOutDir: false,
      // Node 22 and bun both run ES2022.
      target: "es2022",
      // One file: the app host embeds and writes only this one, so a chunk
      // split out for a dynamic import (Windows' private file reader) would
      // never reach the app.
      rollupOptions: { output: { inlineDynamicImports: true } },
      minify: true,
      copyPublicDir: false,
      reportCompressedSize: false,
    },
  })
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const out = process.argv[2]
  if (!out) {
    console.error("usage: app-client-bundle.mjs <out-file>")
    process.exit(2)
  }
  await bundleAppClient(resolve(out))
}
