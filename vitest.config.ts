import { existsSync } from "node:fs"
import { resolve } from "node:path"
import { defineConfig } from "vitest/config"

const designSystemEditorDependencies = existsSync(
  resolve("node_modules/@nessa-ui/react"),
)
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
    dedupe: ["react", "react-dom"],
    alias: [
      // One rule rather than a list of subpaths added as each test needed one.
      // The list is why "a component that imports the design system cannot be
      // tested" was believed: the ninth subpath was simply missing, and the
      // resolution error read like a limitation. `vite.config.ts` has resolved
      // the whole namespace with this regex all along.
      {
        find: /^@nessa-ui\/react\/(.*)$/,
        replacement: resolve("node_modules/@nessa-ui/react/src/components/$1"),
      },
      { find: "@", replacement: resolve("node_modules/@nessa-ui/react/src") },
    ],
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
        // Radix resolves React through the vendored design system's own pnpm
        // store, so left to Node's resolver a component using `Slot` loads a
        // second React and every hook in it throws on a null dispatcher.
        // Transformed here instead, it goes through the dedupe above.
        inline: [/radix-ui/],
      },
    },
  },
})
