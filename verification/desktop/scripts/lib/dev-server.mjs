/**
 * The opt-in isolated-cache child of server.mjs. Load the checkout's ordinary
 * Vite configuration, overriding only the cache and the same listening options
 * as the default CLI launch. The parent owns process exit and cache cleanup.
 */
import { createServer } from "vite"

const server = await createServer({
  cacheDir: process.argv[3],
  server: { host: "127.0.0.1", port: Number(process.argv[2]), strictPort: true },
})
await server.listen()
