/**
 * Browsers and pages: Chromium (installed Google Chrome by default, for
 * Long Animation Frame timing and CDP throttling) and WebKit (the engine
 * WKWebView shares). Every page is seeded with preferences before it loads,
 * made to look like the macOS window (so the safe area includes the traffic
 * lights' inset), and has its console and page errors collected.
 */
import { chromium, webkit } from "playwright"

import { CannotRun } from "./cli.mjs"
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
 */
export async function openPage(browser, o) {
  const context = await browser.newContext({
    viewport: { width: o.width ?? 1440, height: o.height ?? 900 },
    deviceScaleFactor: o.dsf ?? 2,
    reducedMotion: o.reducedMotion,
    colorScheme: o.colorScheme,
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
  } catch (error) {
    // The setup's error is the one reported: a close that fails too is
    // swallowed, so it cannot take its place.
    await context.close().catch(() => {})
    throw error
  }
  const errors = []
  const harmless = []
  page.on("pageerror", (error) => {
    errors.push(`pageerror: ${error.message.split("\n")[0]}`)
  })
  page.on("console", (message) => {
    if (message.type() !== "error") return
    const text = message.text()
    const url = message.location()?.url ?? ""
    const entry = `console.error: ${text.slice(0, 300)}${url ? ` (${url})` : ""}`
    if (harmlessConsole.some((h) => h.text.test(text) && h.url.test(url)))
      harmless.push(entry)
    else errors.push(entry)
  })
  page.on("requestfailed", (request) =>
    recordFailedRequest(request, page.url(), { errors, harmless }),
  )
  try {
    await page.goto(o.url, { waitUntil: "domcontentloaded" })
    await page.waitForSelector(o.readySelector ?? css.anyReady, {
      timeout: o.readyTimeout ?? 30_000,
    })
  } catch (error) {
    await context.close().catch(() => {})
    throw new CannotRun(
      `the desktop page did not render ${o.readySelector ?? css.anyReady} at ${o.url}: ${error.message.split("\n")[0]}` +
        (errors.length ? `\n  page errors: ${errors.join("; ")}` : ""),
    )
  }
  // Ready once its fonts are in and its opening motion has run: a condition,
  // not a guess at how long that takes.
  try {
    await page.waitForFunction(
      () =>
        document.fonts.status === "loaded" &&
        !document
          .getAnimations()
          .some(
            (a) =>
              (a.playState === "running" || a.playState === "pending") &&
              a.effect?.getTiming?.().iterations !== Infinity,
          ),
      null,
      { timeout: o.readyTimeout ?? 30_000, polling: "raf" },
    )
  } catch (error) {
    await context.close().catch(() => {})
    throw new CannotRun(
      `the desktop page never settled at ${o.url}: ${error.message.split("\n")[0]}`,
    )
  }
  return { context, page, errors, harmless, close: () => context.close() }
}

/**
 * Records a request the page reports failed (#485, rows F1–F5; tested in
 * `browser.test.mjs`): a line pushed to `harmless` or to `errors`, or
 * nothing for the fresh browser's `/favicon.ico`.
 *
 * Harmless only when Chromium reports `net::ERR_ABORTED` for the window's own
 * `GET /mcp-resources` (that URL exactly: the client sends its ticket in a
 * header, `fetchMcpResource`) after a 200 arrived (F1). The client reads that
 * body through a reader bounded to its size + 1, and Chromium can report such
 * a request aborted after the whole body reached the page. This function
 * knows only that a 200 arrived: whether the bytes were the right ones is
 * `mcp-apps-gateway.mjs`'s `renders`, which asserts the app is live with the
 * server's document and result. With no response (F2), another status (F3),
 * another URL or error (F4), or a query or fragment (F5), it is an error.
 *
 * `existingResponse()` answers synchronously, so the line is recorded within
 * the event, before a step reads `errors`. Playwright creates a failed
 * request's received response before it reports the failure
 * (`RequestDispatcher`'s constructor in playwright-core).
 *
 * @param {{ url(): string, failure(): { errorText: string } | null,
 *   existingResponse(): { status(): number } | null }} request
 * @param {string} pageUrl the page's URL when the request failed
 * @param {{ errors: string[], harmless: string[] }} into
 */
export function recordFailedRequest(request, pageUrl, { errors, harmless }) {
  const url = request.url()
  if (/favicon\.ico/.test(url)) return
  const errorText = request.failure()?.errorText ?? ""
  const line = `requestfailed: ${url} ${errorText}`
  // The window's own resource URL; none for a page with no origin to resolve it on.
  const resource = URL.canParse("/mcp-resources", pageUrl)
    ? new URL("/mcp-resources", pageUrl).href
    : null
  if (
    errorText === "net::ERR_ABORTED" &&
    url === resource &&
    request.existingResponse()?.status() === 200
  )
    harmless.push(`${line} (aborted after a 200; the bytes are checked by renders, #485)`)
  else errors.push(line)
}

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
    try {
      await body(engine, browser)
    } finally {
      await browser.close()
    }
  }
}
