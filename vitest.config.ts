import { existsSync } from "node:fs"
import { resolve } from "node:path"
import { defineConfig } from "vitest/config"

import {
  linkedSourceRoot,
  sharedPackages,
  viteAliases,
} from "./scripts/nessa-ui-paths.mjs"

const designSystemEditorDependencies = existsSync(resolve("node_modules/@nessa-ui/react"))
  ? [
      "@nessa-ui/react > @tiptap/core",
      "@nessa-ui/react > @tiptap/react",
      "@nessa-ui/react > @tiptap/pm/model",
      "@nessa-ui/react > @tiptap/pm/state",
      "@nessa-ui/react > @tiptap/pm/view",
      "@nessa-ui/react > @tiptap/extension-code-block",
      "@nessa-ui/react > @tiptap/starter-kit",
      "@nessa-ui/react > @tiptap/markdown",
      "@nessa-ui/react > @tiptap/extension-task-list",
      "@nessa-ui/react > @tiptap/extension-task-item",
      "@nessa-ui/react > @tiptap/extension-table",
      "@nessa-ui/react > @tiptap/extension-image",
    ]
  : []

export default defineConfig({
  resolve: {
    dedupe: [...sharedPackages],
    // The design system's import paths, from the one table
    // (`scripts/nessa-ui-paths.mjs`) that `vite.config.ts` uses too, against
    // the source as the `node_modules` link reaches it.
    alias: viteAliases(resolve(linkedSourceRoot)),
  },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx", "packages/**/*.test.ts"],
    environment: "node",
    deps: {
      optimizer: {
        web: {
          enabled: true,
          include: designSystemEditorDependencies,
        },
      },
    },
    server: {
      deps: {
        // Radix, and the Floating UI it positions popovers with, resolve React
        // through the vendored design system's own pnpm store, so left to
        // Node's resolver a component using `Slot` loads a second React and
        // every hook in it throws on a null dispatcher. Transformed here
        // instead, they go through the dedupe above. Everything from that
        // store is inlined, rather than each package as a test meets it; a
        // CommonJS build (react-remove-scroll's, under a modal menu) is
        // required by Node regardless and still loads its own.
        inline: [/radix-ui/, /@floating-ui/, /nessa_ui\/node_modules\//],
      },
    },
  },
})
