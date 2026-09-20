import { resolve } from "node:path"
import { defineConfig } from "vitest/config"

export default defineConfig({
  resolve: {
    dedupe: ["react", "react-dom"],
    alias: {
      "@nessa-ui/react/json-tree": resolve(
        "node_modules/@nessa-ui/react/src/components/json-tree.tsx",
      ),
      "@nessa-ui/react/file-preview": resolve(
        "node_modules/@nessa-ui/react/src/components/file-preview/index.ts",
      ),
      "@": resolve("node_modules/@nessa-ui/react/src"),
      "@nessa-ui/react/message-markdown": resolve(
        "node_modules/@nessa-ui/react/src/components/message-markdown.tsx",
      ),
      "@nessa-ui/react/morphing-mesh-gradient": resolve(
        "node_modules/@nessa-ui/react/src/components/morphing-mesh-gradient.tsx",
      ),
      "@nessa-ui/react/button": resolve(
        "node_modules/@nessa-ui/react/src/components/button.tsx",
      ),
      "@nessa-ui/react/chat-bubbles": resolve(
        "node_modules/@nessa-ui/react/src/components/chat-bubbles.tsx",
      ),
      "@nessa-ui/react/agent-notification": resolve(
        "node_modules/@nessa-ui/react/src/components/agent-notification.tsx",
      ),
      "@nessa-ui/react/file-drop-zone": resolve(
        "node_modules/@nessa-ui/react/src/components/file-drop-zone.tsx",
      ),
    },
  },
  test: {
    include: ["src/**/*.test.ts", "packages/**/*.test.ts"],
    environment: "node",
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
