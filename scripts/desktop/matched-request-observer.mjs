// Temporary observation only: no request routing or assertion classification.
import { writeFileSync } from "node:fs"
import { createRequire } from "node:module"
import { basename, resolve } from "node:path"

const paths = new Set(["/mcp-resources", "/browser/check", "/desktop.html"])
const kinds = new Set(["document", "fetch", "xhr", "script", "stylesheet", "image"])
const number = (value) => (Number.isSafeInteger(value) && value >= 0 ? value : null)

export function facts(request, page, closing, engine) {
  const url = new URL(request.url())
  const own = new URL(page.url())
  const response = request.existingResponse()
  const headers = response?.headers?.() ?? {}
  const length = Object.hasOwn(headers, "content-length")
    ? headers["content-length"]
    : undefined
  let frame = "unknown"
  try {
    frame = request.frame() === page.mainFrame() ? "main" : "child"
  } catch {}
  return {
    engine,
    endpoint: paths.has(url.pathname) ? url.pathname : "other",
    sameOrigin: url.origin === own.origin,
    loopback: ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname),
    method: ["GET", "POST", "OPTIONS"].includes(request.method())
      ? request.method()
      : "other",
    resource: kinds.has(request.resourceType()) ? request.resourceType() : "other",
    frame,
    duringClose: closing,
    pageClosed: page.isClosed(),
    status: number(response?.status()),
    contentLength:
      typeof length === "string" && /^\d{1,12}$/.test(length)
        ? number(Number(length))
        : null,
    bodySize: null,
    sizeReported: false,
    aborted: request.failure()?.errorText === "net::ERR_ABORTED",
  }
}

export function observe(type, engine, records) {
  const launch = type.launch
  type.launch = async function (...args) {
    const browser = await launch.apply(this, args)
    const newContext = browser.newContext
    browser.newContext = async function (...contextArgs) {
      const context = await newContext.apply(this, contextArgs)
      let closing = false
      const close = context.close
      context.close = function (...closeArgs) {
        closing = true
        return close.apply(this, closeArgs)
      }
      context.on("page", (page) => {
        const keep = (request, failed) => {
          if (records.length >= 64) return
          try {
            const pathname = new URL(request.url()).pathname
            if (!failed && !paths.has(pathname)) return
            const entry = {
              event: failed ? "failed" : "finished",
              ...facts(request, page, closing, engine),
            }
            records.push(entry)
            try {
              Promise.resolve(request.sizes())
                .then((sizes) => {
                  entry.bodySize = number(sizes?.responseBodySize)
                  entry.sizeReported = true
                })
                .catch(() => {})
            } catch {}
          } catch {
            records.push({ event: "capture-failed", engine })
          }
        }
        page.on("requestfailed", (request) => keep(request, true))
        page.on("requestfinished", (request) => keep(request, false))
      })
      return context
    }
    return browser
  }
}

// NODE_OPTIONS reaches child tools too. Only gateway-window loads Playwright.
if (
  process.env.MATCHED_REQUEST_OUTPUT &&
  basename(process.argv[1] ?? "") === "gateway-window.mjs"
) {
  const requests = []
  let setup = false
  try {
    const playwright = createRequire(resolve(process.cwd(), "package.json"))("playwright")
    for (const engine of ["chromium", "webkit"])
      observe(playwright[engine], engine, requests)
    setup = true
  } catch {
    // A diagnostic setup fault must not replace the script's own execution.
  } finally {
    process.once("exit", () => {
      try {
        writeFileSync(
          process.env.MATCHED_REQUEST_OUTPUT,
          JSON.stringify({ setup, requests }) + "\n",
        )
      } catch {}
    })
  }
}
