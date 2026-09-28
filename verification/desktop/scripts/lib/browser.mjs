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
function seed([prefs, mac]) {
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
      document.querySelectorAll("[data-surface]").forEach((e) => {
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
 * @param {boolean} [o.overview] turn on the Agents overview preference (default true)
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
    ...(o.overview === false ? {} : { [storage.agentsOverview]: "on" }),
    ...(o.prefs ?? {}),
  }
  await context.addInitScript(seed, [prefs, o.mac !== false])
  for (const script of o.initScripts ?? []) {
    if (Array.isArray(script)) await context.addInitScript(script[0], script[1])
    else await context.addInitScript(script)
  }
  const page = await context.newPage()
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
  page.on("requestfailed", (request) => {
    const url = request.url()
    if (!/favicon\.ico/.test(url))
      errors.push(`requestfailed: ${url} ${request.failure()?.errorText ?? ""}`)
  })
  try {
    await page.goto(o.url, { waitUntil: "domcontentloaded" })
    await page.waitForSelector(o.readySelector ?? css.anyReady, {
      timeout: o.readyTimeout ?? 30_000,
    })
  } catch (error) {
    await context.close()
    throw new CannotRun(
      `the desktop page did not render ${o.readySelector ?? css.anyReady} at ${o.url}: ${error.message.split("\n")[0]}` +
        (errors.length ? `\n  page errors: ${errors.join("; ")}` : ""),
    )
  }
  await page.waitForTimeout(o.settle ?? 1000)
  return { context, page, errors, harmless, close: () => context.close() }
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
