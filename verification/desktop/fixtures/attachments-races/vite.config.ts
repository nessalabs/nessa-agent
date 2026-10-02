/** Serves the real panel with controlled host effects and scenario gateway/session effects. */
import { defineConfig } from "vite"
import base from "../../../../vite.config"

const replaced = [
  "hasNativeHost",
  "chooseAttachmentFiles",
  "readAttachmentBytes",
  "onAttachmentReadying",
  "onAttachmentBatch",
  "onAttachmentDragging",
  "onAttachmentDropped",
]
export default defineConfig({
  ...base,
  root: new URL("../../../..", import.meta.url).pathname,
  plugins: [
    ...(base.plugins ?? []),
    {
      name: "attachment-verification-host-effects",
      enforce: "pre",
      transform(code, id) {
        if (!id.endsWith("/src/host/index.ts")) return
        for (const name of replaced)
          code = code.replace(new RegExp(`^  ${name},\\n`, "m"), "")
        return (
          code +
          `\nexport const hasNativeHost = () => true;
export const chooseAttachmentFiles = () => window.__attachmentProbe.choose();
export const readAttachmentBytes = (ticket) => window.__attachmentProbe.read(ticket);
export const onAttachmentReadying = () => Promise.resolve(() => {});
export const onAttachmentBatch = () => Promise.resolve(() => {});
export const onAttachmentDragging = () => Promise.resolve(() => {});
export const onAttachmentDropped = (handler) => { window.__attachmentProbe.drop = handler; return Promise.resolve(() => {}); };`
        )
      },
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          if (!request.url?.startsWith("/attachment-verification")) return next()
          response.setHeader("Content-Type", "text/html")
          response.end(
            await server.transformIndexHtml(
              request.url,
              '<!doctype html><html><body><div id="root"></div><script type="module" src="/verification/desktop/fixtures/attachments-races/main.tsx"></script></body></html>',
            ),
          )
        })
      },
    },
  ],
  server: { ...base.server, host: "127.0.0.1", port: 1448, strictPort: true },
})
