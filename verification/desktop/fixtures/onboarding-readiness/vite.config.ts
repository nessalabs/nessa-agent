/** Real readiness HTTP responses: no headers, an incomplete body, then recovery. */
import { defineConfig } from "vite"
import base from "../../../../vite.config"

export default defineConfig({
  ...base,
  root: new URL("../../../..", import.meta.url).pathname,
  plugins: [
    ...(base.plugins ?? []),
    {
      name: "onboarding-readiness-verification",
      configureServer(server) {
        const counts = new Map<string, number>()
        server.middlewares.use(async (request, response, next) => {
          const url = new URL(request.url ?? "/", "http://fixture")
          if (url.pathname === "/onboarding-readiness") {
            response.setHeader("Content-Type", "text/html")
            response.end(
              await server.transformIndexHtml(
                url.pathname,
                '<!doctype html><html><body><div id="root"></div><script type="module" src="/verification/desktop/fixtures/onboarding-readiness/main.tsx"></script></body></html>',
              ),
            )
            return
          }
          const route =
            /^\/readiness-probe\/([^/]+)\/(fetch|body)\/onboarding\/agents$/.exec(
              url.pathname,
            )
          if (!route) return next()
          const [, token, phase] = route
          const call = (counts.get(token) ?? 0) + 1
          counts.set(token, call)
          if (call === 1) {
            if (phase === "body") {
              response.setHeader("Content-Type", "application/json")
              response.write('{"agents":[')
            }
            // Neither response ends: the real browser's AbortSignal must stop it.
            return
          }
          response.setHeader("Content-Type", "application/json")
          response.end(JSON.stringify({ agents: [{ id: "claude", readiness: "ready" }] }))
        })
      },
    },
  ],
  server: { ...base.server, host: "127.0.0.1", port: 0 },
})
