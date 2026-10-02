/**
 * The sandbox proxy's origin in the browser build (`pnpm dev`, `just web`):
 * a listener of its own, beside Vite's, on a port the OS
 * chooses, serving `proxy.html` at `/proxy.html` and nothing else — no app
 * page, no gateway proxy — so nothing an app could reach on that origin is
 * the window's. The desktop app serves the same file on its `nessa-sandbox`
 * scheme instead (`src-tauri/src/app_sandbox.rs`).
 *
 * The page learns where the proxy is from a `<meta name="nessa-app-sandbox">`
 * this plugin writes into each HTML page it serves; composition reads it
 * (`adapters/dom/sandbox-origin.ts`). Over HTTPS the listener uses Vite's
 * own certificate, so the frame is never mixed content.
 *
 * `vite preview` serves a build as it was written, so a previewed page has
 * no meta and shows every app as one it cannot load: the preview is for the
 * frame budget (`verification/desktop`), which no app is part of.
 */
import { readFileSync } from "node:fs"
import {
  createServer as createHttpServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http"
import { createServer as createHttpsServer, type ServerOptions } from "node:https"
import type { AddressInfo } from "node:net"
import { fileURLToPath } from "node:url"
import type { Plugin } from "vite"
import { sandboxMetaName } from "../adapters/dom/sandbox-origin"

const proxyPath = "/proxy.html"
const host = "127.0.0.1"

function proxyDocument(): Buffer {
  return readFileSync(fileURLToPath(new URL("./proxy.html", import.meta.url)))
}

function answer(request: IncomingMessage, response: ServerResponse) {
  const path = new URL(request.url ?? "/", "http://sandbox").pathname
  if (request.method !== "GET" || path !== proxyPath) {
    response.writeHead(404, { "Content-Type": "text/plain" }).end("Not found")
    return
  }
  response
    .writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      "Cache-Control": "no-store",
      "X-Content-Type-Options": "nosniff",
    })
    .end(proxyDocument())
}

/** Starts the listener; resolves with the proxy's URL. */
function listen(
  https: ServerOptions | undefined,
): Promise<{ url: string; close(): void }> {
  const server = https ? createHttpsServer(https, answer) : createHttpServer(answer)
  return new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(0, host, () => {
      const { port } = server.address() as AddressInfo
      resolve({
        url: `${https ? "https" : "http"}://${host}:${port}${proxyPath}`,
        close: () => server.close(),
      })
    })
  })
}

function withMeta(html: string, url: string): string {
  const meta = `<meta name="${sandboxMetaName}" content="${url}">`
  return html.includes("</head>")
    ? html.replace("</head>", `${meta}</head>`)
    : meta + html
}

/** The Vite plugin: the listener, for the dev server's life. */
export function appSandbox(): Plugin {
  let url: string | undefined
  return {
    name: "nessa-app-sandbox",
    async configureServer(server) {
      const https = server.config.server.https || undefined
      const listener = await listen(https)
      url = listener.url
      server.httpServer?.once("close", listener.close)
    },
    transformIndexHtml: {
      order: "post",
      handler: (html) => (url ? withMeta(html, url) : html),
    },
  }
}
