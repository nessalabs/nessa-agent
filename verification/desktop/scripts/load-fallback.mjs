#!/usr/bin/env node
/**
 * The load fallback in `index.html`: what the panel and setup show before the
 * frontend mounts, measured with the frontend held back so it never does.
 *
 * On macOS and Linux the panel's webview is larger than its window and pinned
 * to the window's bottom right (`src/panel/adapters/panel-frame.ts`), so the
 * viewport is the stage and only its bottom-right corner is on screen. Each
 * scenario gives the page a stage and a host that reports the window's size
 * (or never does, or refuses), then checks that the avatar and "Loading" sit
 * wholly inside the visible window, centred in it once its size is known, and
 * that nothing paints over them.
 */
import { launch } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

/** `host`: a window size the host reports, "pending", "refused", or null for a plain browser. */
const scenarios = [
  {
    name: "panel, default frame",
    surface: "panel",
    stage: { width: 1440, height: 875 },
    host: { width: 420, height: 835 },
    centred: true,
  },
  {
    name: "panel, configured short",
    surface: "panel",
    stage: { width: 1920, height: 1055 },
    host: { width: 420, height: 320 },
    centred: true,
  },
  {
    name: "panel, narrow",
    surface: "panel",
    stage: { width: 1280, height: 695 },
    host: { width: 300, height: 655 },
    centred: true,
  },
  {
    name: "panel, size not yet reported",
    surface: "panel",
    stage: { width: 1440, height: 875 },
    host: "pending",
    visible: { width: 420, height: 320 },
  },
  {
    name: "panel, size refused",
    surface: "panel",
    stage: { width: 1440, height: 875 },
    host: "refused",
    visible: { width: 420, height: 320 },
  },
  {
    name: "panel, plain browser",
    surface: "panel",
    stage: { width: 420, height: 800 },
    host: null,
    centred: true,
  },
  {
    name: "setup",
    surface: "setup",
    stage: { width: 1000, height: 700 },
    host: null,
    centred: true,
  },
]

/** Installed before the page's own scripts, as Tauri installs its IPC. */
function fakeHost(host) {
  if (host === null) return
  window.__TAURI_INTERNALS__ = {
    invoke(command) {
      if (command !== "panel_size") return Promise.reject(new Error(command))
      if (host === "pending") return new Promise(() => {})
      if (host === "refused") return Promise.reject(new Error("refused"))
      return Promise.resolve(host)
    },
  }
}

async function measure(page) {
  return page.evaluate(
    ([message, mark, title]) => {
      const rect = (selector) => {
        const r = document.querySelector(selector).getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const t = rect(title)
      const covering = document.elementFromPoint(
        (t.left + t.right) / 2,
        (t.top + t.bottom) / 2,
      )
      return {
        message: rect(message),
        mark: rect(mark),
        title: t,
        text: document.querySelector(message).textContent.trim(),
        titleOnTop: covering === document.querySelector(title),
        animations: document.getAnimations().length,
        overflow:
          document.documentElement.scrollWidth > innerWidth ||
          document.documentElement.scrollHeight > innerHeight,
      }
    },
    [css.loadMessage, css.loadMark, css.loadTitle],
  )
}

function check(scenario, m) {
  const failures = []
  const { stage } = scenario
  const window =
    typeof scenario.host === "object" && scenario.host ? scenario.host : scenario.visible
  // The window is pinned to the stage's bottom right; a plain viewport is all window.
  const clip = window
    ? {
        left: stage.width - window.width,
        top: stage.height - window.height,
        right: stage.width,
        bottom: stage.height,
      }
    : { left: 0, top: 0, right: stage.width, bottom: stage.height }
  const content = {
    left: Math.min(m.mark.left, m.title.left),
    top: m.mark.top,
    right: Math.max(m.mark.right, m.title.right),
    bottom: m.title.bottom,
  }
  const inside =
    content.left >= clip.left - 0.5 &&
    content.top >= clip.top - 0.5 &&
    content.right <= clip.right + 0.5 &&
    content.bottom <= clip.bottom + 0.5
  if (!inside)
    failures.push(
      `avatar and title ${JSON.stringify(content)} are not inside the visible window ${JSON.stringify(clip)}`,
    )
  if (scenario.centred) {
    const dx = (content.left + content.right) / 2 - (clip.left + clip.right) / 2
    const dy = (content.top + content.bottom) / 2 - (clip.top + clip.bottom) / 2
    if (Math.abs(dx) > 1 || Math.abs(dy) > 1)
      failures.push(
        `off centre of the visible window by ${dx.toFixed(1)}, ${dy.toFixed(1)}`,
      )
  }
  if (m.text !== "Loading") failures.push(`says ${JSON.stringify(m.text)}, not "Loading"`)
  if (!m.titleOnTop) failures.push("something paints over the title")
  if (m.overflow) failures.push("the page scrolls")
  return { failures, measured: { clip, content } }
}

await main(
  {
    name: "load-fallback",
    summary: "the load fallback sits inside, and centred in, the visible window",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
    const origin = new URL(url).origin
    for (const engine of options.engines) {
      let browser
      try {
        browser = await launch(engine, options)
      } catch (error) {
        rep.add({ name: "launch", engine, cannotRun: true, error: error.message })
        continue
      }
      try {
        for (const motion of ["no-preference", "reduce"])
          for (const scenario of scenarios) {
            if (motion === "reduce" && scenario.name !== "panel, default frame") continue
            const name = `${scenario.name}${motion === "reduce" ? ", reduced motion" : ""}`
            await attempt(
              rep,
              { name, engine, width: scenario.stage.width },
              async () => {
                const context = await browser.newContext({
                  viewport: scenario.stage,
                  deviceScaleFactor: 2,
                  reducedMotion: motion,
                })
                try {
                  await context.addInitScript(fakeHost, scenario.host)
                  const page = await context.newPage()
                  const errors = []
                  const held = new Set()
                  const heldUrls = new Set()
                  page.on("pageerror", (error) =>
                    errors.push(`pageerror: ${error.message.split("\n")[0]}`),
                  )
                  page.on("console", (message) => {
                    if (message.type() !== "error") return
                    // Chromium reports each held-back script once more, as a
                    // failed resource at that script's URL.
                    if (heldUrls.has(message.location()?.url ?? "")) return
                    errors.push(`console.error: ${message.text().slice(0, 300)}`)
                  })
                  // Hold the frontend back so the fallback is what stays: every
                  // script the page loads, whether dev's /src/main.tsx or a
                  // production build's hashed entry. The bootstrap is inline.
                  await page.route("**/*", (route) => {
                    if (route.request().resourceType() !== "script")
                      return route.continue()
                    held.add(route.request())
                    heldUrls.add(route.request().url())
                    return route.abort()
                  })
                  page.on("requestfailed", (request) => {
                    if (held.has(request) || /favicon\.ico/.test(request.url())) return
                    errors.push(
                      `requestfailed: ${request.url()} ${request.failure()?.errorText ?? ""}`,
                    )
                  })
                  const search = scenario.surface === "setup" ? "?surface=setup" : ""
                  await page.goto(`${origin}/index.html${search}`)
                  await page.waitForSelector(css.loadMark)
                  // The reported size lands a microtask after the bootstrap runs.
                  await page.evaluate(() => new Promise(requestAnimationFrame))
                  const m = await measure(page)
                  const result = check(scenario, m)
                  if (held.size === 0)
                    result.failures.push("no frontend script was held back")
                  result.failures.push(...errors)
                  if (motion === "reduce" && m.animations !== 0)
                    result.failures.push(
                      `${m.animations} animations run with reduced motion`,
                    )
                  return result
                } finally {
                  await context.close()
                }
              },
            )
          }
      } finally {
        await browser.close()
      }
    }
  },
)
