import { resolve } from "node:path"
import { defineConfig } from "vitest/config"

export default defineConfig({
  resolve: {
    alias: {
      "@": resolve("node_modules/@nessa-ui/react/src"),
      "@nessa-ui/react/message-markdown": resolve(
        "node_modules/@nessa-ui/react/src/components/message-markdown.tsx",
      ),
      "@nessa-ui/react/button": resolve(
        "node_modules/@nessa-ui/react/src/components/button.tsx",
      ),
    },
  },
  test: {
    include: ["src/**/*.test.ts", "packages/**/*.test.ts"],
    environment: "node",
  },
})
