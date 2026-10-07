/**
 * Browsers and pages: Chromium (installed Google Chrome by default, for
 * Long Animation Frame timing and CDP throttling) and WebKit (the engine
 * WKWebView shares). Every page is seeded with preferences before it loads,
 * made to look like the macOS window (so the safe area includes the traffic
 * lights' inset), and has its console and page errors collected.
 */
import { chromium, webkit } from "playwright"

import { CannotRun } from "./cli.mjs"
import { attachLines, reportDetached, reporterBound, watchLines } from "./page-lines.mjs"
import { harmlessConsole, css, storage } from "./selectors.mjs"

const engines = { chromium, webkit }

/** Launches one engine. `channel: "bundled"` uses Playwright's own Chromium. */
export async function launch(engine, options) {
  const type = engines[engine]
  if (!type) throw new CannotRun(`unknown engine ${engine} (chromium or webkit)`)
  const launchOptions = { headless: !options.headed }
  if (options.env) launchOptions.env = options.env
  if (engine === "chromium") {
    if (options.channel && options.channel !== "bundled")
      launchOptions.channel = options.channel
    launchOptions.args = ["--enable-blink-features=LongAnimationFrameTiming"]
  }
  try {
    return await type.launch(launchOptions)
  } catch (error) {
    throw new CannotRun(
      `${engine} did not launch (${error.message.split("\n")[0]}). ` +
        (engine === "webkit" || options.channel === "bundled"
          ? "Run: pnpm exec playwright install " +
            (engine === "webkit" ? "webkit" : "chromium")
          : "Install Google Chrome, or pass --channel bundled after pnpm exec playwright install chromium"),
    )
  }
}

/**
 * Seeds localStorage once per context (a reload keeps what the page changed)
 * and marks every surface as the macOS window.
 */
function seed([prefs, mac, surface]) {
  try {
    if (!sessionStorage.getItem("__verify_seeded")) {
      for (const [key, value] of Object.entries(prefs)) localStorage.setItem(key, value)
      sessionStorage.setItem("__verify_seeded", "1")
    }
  } catch {
    // Storage can be unavailable on an about:blank frame; the page's own frame is what matters.
  }
  if (mac) {
    const mark = () =>
      document.querySelectorAll(surface).forEach((e) => {
        if (e.getAttribute("data-host") !== "macos") e.setAttribute("data-host", "macos")
        if (e.getAttribute("data-surface") !== "window")
          e.setAttribute("data-surface", "window")
      })
    new MutationObserver(mark).observe(document, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["data-host", "data-surface"],
    })
  }
}

/**
 * The page has its fonts and its opening motion has finished. Infinite
 * animations (the ambient grain, a spinner that does not stop) do not hold
 * this open. Passed to `page.waitForFunction`, so it cannot close over the
 * module.
 */
export function documentSettled() {
  return (
    document.fonts.status === "loaded" &&
    !document
      .getAnimations()
      .some(
        (animation) =>
          (animation.playState === "running" || animation.playState === "pending") &&
          animation.effect?.getTiming?.().iterations !== Infinity,
      )
  )
}

/** Waits until `documentSettled` holds. `timeout` is the same bound `openPage` uses. */
export async function waitUntilSettled(page, timeout = 30_000) {
  await page.waitForFunction(documentSettled, null, { timeout, polling: "raf" })
}

/**
 * Opens the desktop page in a fresh context.
 *
 * @param {import("playwright").Browser} browser
 * @param {object} o
 * @param {string} o.url
 * @param {string} [o.layout] columns | sidebar | classic
 * @param {number} [o.width] @param {number} [o.height]
 * @param {"reduce"|"no-preference"} [o.reducedMotion]
 * @param {Record<string,string>} [o.prefs] extra localStorage entries
 * @param {Array<Function|[Function, unknown]>} [o.initScripts] extra init scripts
 * @param {(page: import("playwright").Page, context: import("playwright").BrowserContext) => Promise<void>} [o.preparePage]
 *        Runs after the page exists and before navigation, so a CDP session
 *        can disable the cache or enable performance metrics for that load.
 */
export async function openPage(browser, o) {
  const context = await browser.newContext({
    viewport: { width: o.width ?? 1440, height: o.height ?? 900 },
    deviceScaleFactor: o.dsf ?? 2,
    reducedMotion: o.reducedMotion,
    colorScheme: o.colorScheme,
    // The harness's chords are ⌘ and the window is marked macOS. Chrome on
    // Linux would otherwise treat Control as the command key.
    userAgent:
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
  })
  const prefs = {
    [storage.layout]: o.layout ?? "columns",
    ...(o.prefs ?? {}),
  }
  let page
  try {
    await context.addInitScript(seed, [prefs, o.mac !== false, css.surface])
    for (const script of o.initScripts ?? []) {
      if (Array.isArray(script)) await context.addInitScript(script[0], script[1])
      else await context.addInitScript(script)
    }
    // What a check sets up before the page loads, such as a socket it
    // answers or the clock it holds.
    await o.beforeLoad?.(context)
    page = await context.newPage()
    if (o.preparePage) await o.preparePage(page, context)
  } catch (error) {
    // The setup's error is the one reported: a close that fails too is
    // swallowed, so it cannot take its place.
    await context.close().catch(() => {})
    throw error
  }
  const lines = watchLines()
  page.on("pageerror", (error) => {
    lines.keep(`pageerror: ${error.message.split("\n")[0]}`, false)
  })
  page.on("console", (message) => {
    if (message.type() !== "error") return
    const text = message.text()
    const url = message.location()?.url ?? ""
    const entry = `console.error: ${text.slice(0, 300)}${url ? ` (${url})` : ""}`
    lines.keep(
      entry,
      harmlessConsole.some((h) => h.text.test(text) && h.url.test(url)),
    )
  })
  const sizeReports = []
  page.on("requestfailed", (request) => {
    const pageUrl = page.url()
    const bucket = { errors: [], harmless: [] }
    recordFailedRequest(request, pageUrl, bucket)
    for (const line of bucket.harmless) lines.keep(line, true)
    for (const line of bucket.errors) lines.keep(line, false)
    // `sizes()` asks the browser and answers later. The caller awaits
    // `settleRequests` before a result takes `errors`, so a full body can
    // still leave `errors` (#473). A report that never arrives leaves the line.
    const pending = typeof request.sizes === "function" ? request.sizes() : null
    if (!pending || typeof pending.then !== "function") return
    // Kept as a report, not a bare promise: a size that arrives after the
    // step stopped waiting must not move a later line with the same text.
    sizeReports.push(
      enqueueSizeReport(pending, (sizes) =>
        reclassifyDeliveredAbort(request, pageUrl, sizes, {
          errors: lines.errors,
          harmless: lines.harmless,
        }),
      ),
    )
  })
  const abandon = async (message, includeErrors) => {
    // A size report can still move a full-body abort before the lines are
    // reported. Closing first would drop that report.
    await settleSizeReports(sizeReports)
    await context.close().catch(() => {})
    if (reporterBound()) {
      reportDetached(o.lines ?? {}, lines.errors, lines.harmless)
      throw new CannotRun(message)
    }
    const extra =
      includeErrors && lines.errors.length
        ? `\n  page errors: ${lines.errors.join("; ")}`
        : ""
    throw new CannotRun(message + extra)
  }
  try {
    await page.goto(o.url, { waitUntil: "domcontentloaded" })
    await page.waitForSelector(o.readySelector ?? css.anyReady, {
      timeout: o.readyTimeout ?? 30_000,
    })
  } catch (error) {
    await abandon(
      `the desktop page did not render ${o.readySelector ?? css.anyReady} at ${o.url}: ${error.message.split("\n")[0]}`,
      true,
    )
  }
  // Ready once its fonts are in and its opening motion has run: a condition,
  // not a guess at how long that takes.
  try {
    await waitUntilSettled(page, o.readyTimeout ?? 30_000)
  } catch (error) {
    await abandon(
      `the desktop page never settled at ${o.url}: ${error.message.split("\n")[0]}`,
      false,
    )
  }
  const opened = {
    context,
    page,
    errors: lines.errors,
    harmless: lines.harmless,
    noteHarmless: (pattern) => lines.noteHarmless(pattern),
    reclassifyHeld: (keep, asHarmless) => lines.reclassifyHeld(keep, asHarmless),
    close: () => context.close(),
    settleRequests: () => settleSizeReports(sizeReports),
  }
  attachLines(opened, o.lines ?? {})
  return opened
}

/**
 * Remembers one `sizes()` answer. `apply` runs only if `settleSizeReports`
 * is still waiting for it. A report abandoned at the deadline cannot move a
 * line recorded later (#473).
 *
 * @param {Promise<unknown>} sizes
 * @param {(sizes: unknown) => void} apply
 * @returns {{ abandoned: boolean, settled: Promise<void> }}
 */
export function enqueueSizeReport(sizes, apply) {
  const report = { abandoned: false, settled: /** @type {Promise<void>} */ (null) }
  report.settled = sizes
    .then((value) => {
      if (report.abandoned) return
      apply(value)
    })
    .catch(() => {})
  return report
}

/**
 * Waits for size reports already asked of the browser, so a caller can read
 * `errors` after a full body has been moved out. A report still pending when
 * `timeoutMs` elapses is abandoned: it stays an error, and its later answer
 * does not change `errors` (#473).
 *
 * @param {Array<Promise<unknown> | { abandoned?: boolean, settled: Promise<unknown> }>} pending
 * @param {number} [timeoutMs]
 */
export async function settleSizeReports(pending, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs
  while (pending.length > 0 && Date.now() < deadline) {
    const batch = pending.splice(0)
    let timedOut = false
    let timer
    try {
      await Promise.race([
        Promise.all(batch.map((item) => item.settled ?? item)),
        new Promise((resolve) => {
          timer = setTimeout(
            () => {
              timedOut = true
              resolve()
            },
            Math.max(0, deadline - Date.now()),
          )
        }),
      ])
    } finally {
      clearTimeout(timer)
    }
    if (timedOut) {
      abandonSizeReports(batch)
      abandonSizeReports(pending.splice(0))
      return
    }
  }
  if (Date.now() >= deadline) abandonSizeReports(pending.splice(0))
}

function abandonSizeReports(reports) {
  for (const report of reports) {
    if (report && Object.hasOwn(report, "abandoned")) report.abandoned = true
  }
}

/**
 * Records a request the page reports failed (#473, #485; tested in
 * `browser.test.mjs`): a line pushed to `harmless` or to `errors`, or
 * nothing for the fresh browser's `/favicon.ico`.
 *
 * `net::ERR_ABORTED` is harmless only in two cases, never for every abort:
 *
 * - a 200 on the page's own origin whose `content-length` was fully
 *   delivered (`sizes().responseBodySize`). Chromium sometimes reports
 *   that fetch as aborted after the page has read it (#473). Without
 *   those sizes the line stays an error; a caller that can see the mount
 *   went live may move it (`liveMountResourceAbort`).
 * - a 204 on the page's own origin, whose body is empty and never read
 *   (`/browser/check`, #485).
 *
 * With no response, a short body, a status other than those two, another
 * origin, another error text, or a page with no origin, it is an error.
 *
 * `existingResponse()` answers synchronously, so the line is recorded within
 * the event, before a step reads `errors`. Playwright creates a failed
 * request's received response before it reports the failure
 * (`RequestDispatcher`'s constructor in playwright-core). `sizes()` may
 * answer later; `reclassifyDeliveredAbort` moves the line then.
 *
 * @param {{ url(): string, failure(): { errorText: string } | null,
 *   existingResponse(): { status(): number, headers?: () => Record<string, string> } | null,
 *   sizes?: () => { responseBodySize: number } | Promise<unknown> }} request
 * @param {string} pageUrl the page's URL when the request failed
 * @param {{ errors: string[], harmless: string[] }} into
 */
export function recordFailedRequest(request, pageUrl, { errors, harmless }) {
  const url = request.url()
  if (/favicon\.ico/.test(url)) return
  const errorText = request.failure()?.errorText ?? ""
  const line = `requestfailed: ${url} ${errorText}`
  const kind = abortKind(request, pageUrl, errorText)
  if (kind === "full")
    harmless.push(`${line} (aborted after a 200 response, full body, #473)`)
  else if (kind === "empty") harmless.push(`${line} (aborted after a 204 response, #485)`)
  else errors.push(line)
}

/**
 * Moves a recorded abort from `errors` to `harmless` once `sizes` shows the
 * 200 body's `content-length` arrived (#473), and only when `abortKind` still
 * says that abort is the harmless one. No-op when the line was already taken,
 * the body is short, the origin differs, or the error is not `net::ERR_ABORTED`.
 *
 * @param {Parameters<typeof recordFailedRequest>[0]} request
 * @param {string} pageUrl
 * @param {{ responseBodySize?: number } | null | undefined} sizes
 * @param {{ errors: string[], harmless: string[] }} into
 */
export function reclassifyDeliveredAbort(request, pageUrl, sizes, { errors, harmless }) {
  const errorText = request.failure()?.errorText ?? ""
  // The late size report does not relax the abort rule. A full 200 on another
  // origin, or a failure that is not `net::ERR_ABORTED`, stays an error.
  if (abortKind(request, pageUrl, errorText, sizes) !== "full") return
  const line = `requestfailed: ${request.url()} ${errorText}`
  const index = errors.indexOf(line)
  if (index === -1) return
  errors.splice(index, 1)
  harmless.push(`${line} (aborted after a 200 response, full body, #473)`)
}

/**
 * The page-wide guard could not see a body size for this `/mcp-resources`
 * abort. A step may treat it as harmless only when the mount that fetched
 * it went live, and the request is on the page's own origin (#473). A
 * cross-origin abort stays a failure.
 *
 * @param {string} line
 * @param {string} pageUrl
 */
export function liveMountResourceAbort(line, pageUrl) {
  const match = /^requestfailed: (\S+\/mcp-resources(?:[?#]\S*)?) net::ERR_ABORTED$/.exec(
    line,
  )
  if (!match) return false
  const own = originOf(pageUrl)
  return own !== "null" && originOf(match[1]) === own
}

/**
 * @param {{ responseBodySize?: number } | null | undefined} [sizes]
 *   The body size already in hand. Omitted, the synchronous answer of
 *   `sizes()` is used, and a promise is not a delivery.
 * @returns {"full" | "empty" | "error"}
 */
function abortKind(request, pageUrl, errorText, sizes = synchronousSizes(request)) {
  if (errorText !== "net::ERR_ABORTED") return "error"
  const url = request.url()
  const own = originOf(pageUrl)
  if (own === "null" || originOf(url) !== own) return "error"
  const response = request.existingResponse()
  const status = response?.status()
  if (status === 204) return "empty"
  if (status === 200 && fullBody(request, sizes)) return "full"
  return "error"
}

function synchronousSizes(request) {
  if (typeof request.sizes !== "function") return null
  const sizes = request.sizes()
  if (!sizes || typeof sizes.then === "function") {
    // The caller asks again and waits. A rejection of this unused answer
    // must not surface as an unhandled rejection.
    if (sizes && typeof sizes.catch === "function") sizes.catch(() => {})
    return null
  }
  return sizes
}

function fullBody(request, sizes) {
  const response = request.existingResponse()
  if (!response || response.status() !== 200) return false
  const headers = typeof response.headers === "function" ? response.headers() : null
  if (!headers || !Object.hasOwn(headers, "content-length")) return false
  const length = Number(headers["content-length"])
  if (!Number.isInteger(length) || length < 0) return false
  const body = sizes?.responseBodySize
  return typeof body === "number" && body >= length
}

/** `url`'s origin, or "null" when it has none to compare. */
const originOf = (url) => (URL.canParse(url) ? new URL(url).origin : "null")

/** Fails with a clear "could not run" when the page lacks what a script needs. */
export async function need(page, selector, what, timeout = 5000) {
  try {
    await page.waitForSelector(selector, { timeout, state: "attached" })
  } catch {
    throw new CannotRun(
      `could not find ${what} (${selector}). The UI may be mid-change; update lib/selectors.mjs if it moved.`,
    )
  }
}

/**
 * Runs `body`, then closes `browser`. The body's error is the one reported:
 * a close that fails is discarded, as `openPage` does when a context fails
 * to close (#475).
 *
 * @param {{ close: () => Promise<unknown> }} browser
 * @param {() => Promise<unknown>} body
 */
export async function runAndClose(browser, body) {
  try {
    return await body()
  } finally {
    await browser.close().catch(() => {})
  }
}

/**
 * Opens a browser per engine, runs `body(engine, browser)`, and always
 * closes it. An engine that does not launch is reported as could-not-run.
 */
export async function withEngines(options, rep, body) {
  for (const engine of options.engines) {
    let browser
    try {
      browser = await launch(engine, options)
    } catch (error) {
      rep.add({ name: "launch", engine, cannotRun: true, error: error.message })
      continue
    }
    await runAndClose(browser, () => body(engine, browser))
  }
}
