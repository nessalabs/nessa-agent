/** Production verification build: same app configuration, with the controlled fixture added. */
import { resolve } from "node:path"
import { mergeConfig } from "vite"
import application from "../../../../vite.config"

export default mergeConfig(application, {
  build: {
    rollupOptions: {
      input: {
        messageSync: resolve("verification/desktop/fixtures/message-sync/index.html"),
      },
    },
  },
})
