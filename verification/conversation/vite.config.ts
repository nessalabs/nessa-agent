/** Production verification build: application configuration plus its controlled conversation fixture. */
import { resolve } from "node:path"
import { mergeConfig } from "vite"
import application from "../../vite.config"

export default mergeConfig(application, {
  build: {
    rollupOptions: {
      input: { conversation: resolve("verification/conversation/fixture.html") },
    },
  },
})
