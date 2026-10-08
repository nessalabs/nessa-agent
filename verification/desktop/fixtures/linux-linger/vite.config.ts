/** Setup's linger step, with a scripted host answer and no logind. */
import { defineConfig } from "vite"
import base from "../../../../vite.config"

export default defineConfig({
  ...base,
  root: new URL("../../../..", import.meta.url).pathname,
  plugins: [
    ...(base.plugins ?? []),
    {
      name: "linux-linger-verification",
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          const url = new URL(request.url ?? "/", "http://fixture")
          if (url.pathname !== "/linux-linger") return next()
          response.setHeader("Content-Type", "text/html")
          response.end(
            await server.transformIndexHtml(
              url.pathname,
              '<!doctype html><html><body><div id="root"></div><script type="module" src="/verification/desktop/fixtures/linux-linger/main.tsx"></script></body></html>',
            ),
          )
        })
      },
    },
  ],
  server: { ...base.server, host: "127.0.0.1", port: 0 },
})
