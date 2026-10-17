/** Production verification adds the controlled Settings fixture to the app inputs. */
import { resolve } from "node:path"
import { mergeConfig } from "vite"
import application from "../../../../vite.config"

export default mergeConfig(application, {
  build: {
    rollupOptions: {
      input: {
        nativeHooks: resolve("verification/desktop/fixtures/native-hooks-on/index.html"),
      },
    },
  },
})
