#!/usr/bin/env node
/**
 * The load fallback in `index.html`: what the panel and setup show before the
 * frontend mounts.
 *
 * On macOS and Linux the panel's webview is larger than its window and pinned
 * to the window's bottom right (`src/panel/adapters/panel-frame.ts`), so the
 * viewport is the stage and only its bottom-right corner is on screen. A
 * native scenario runs the real frontend against a fake host whose startup
 * never answers, so the panel never mounts, and which reports the window's
 * size (or never does, or refuses); `main.tsx` writes that size through
 * `windowSize()` and `publishWindowSize()`. A browser scenario holds every
 * script back instead. A held-back script names the page it could not load.
 * Each other scenario checks that the avatar and "Loading" sit wholly
 * inside the visible window, centred in it once its size is known, and that
 * nothing paints over them.
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
    name: "panel, frontend never loads",
    surface: "panel",
    stage: { width: 420, height: 800 },
    host: null,
  },
  {
    name: "setup",
    surface: "setup",
    stage: { width: 1000, height: 700 },
    host: null,
    centred: true,
  },
]

/**
 * Installed before the page's own scripts, as Tauri installs its IPC. Only
 * `panel_size` is ever answered: startup, and everything else, waits forever.
 */
function fakeHost(host) {
  if (host === null) return
  let callbacks = 0
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => ++callbacks,
    invoke(command) {
      if (command !== "panel_size" || host === "pending") return new Promise(() => {})
      if (host === "refused") return Promise.reject(new Error("refused"))
      return Promise.resolve(host)
    },
  }
}

async function measure(page, phase = null) {
  return page.evaluate(
    async ([message, mark, title, phase]) => {
      const messageElement = document.querySelector(message)
      const markElement = document.querySelector(mark)
      const markAnimation = document
        .getAnimations()
        .find((animation) => animation.effect?.target === markElement)
      const timing = markAnimation?.effect?.getTiming()
      const phaseMilliseconds =
        phase !== null && typeof timing?.duration === "number"
          ? timing.delay + timing.duration * phase
          : null
      if (phaseMilliseconds !== null && markAnimation) {
        markAnimation.pause()
        await markAnimation.ready
        markAnimation.currentTime = phaseMilliseconds
        await new Promise(requestAnimationFrame)
      }
      const rect = (selector) => {
        const r = document.querySelector(selector).getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const messageRect = rect(message)
      // offset geometry belongs to the fixed grid, unaffected by breathing.
      // Its integer rounding is below the existing one-pixel alignment bound.
      const markLayout = {
        left: messageRect.left + markElement.offsetLeft,
        top: messageRect.top + markElement.offsetTop,
        right: messageRect.left + markElement.offsetLeft + markElement.offsetWidth,
        bottom: messageRect.top + markElement.offsetTop + markElement.offsetHeight,
      }
      const t = rect(title)
      const covering = document.elementFromPoint(
        (t.left + t.right) / 2,
        (t.top + t.bottom) / 2,
      )
      return {
        message: messageRect,
        markLayout,
        markLayoutOwned: markElement.offsetParent === messageElement,
        markAnimationPresent: Boolean(markAnimation),
        phase,
        phaseMilliseconds,
        keyframeOffsets: markAnimation?.effect
          ?.getKeyframes()
          .map((frame) => frame.computedOffset),
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
    [css.loadMessage, css.loadMark, css.loadTitle, phase],
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
    top: Math.min(m.mark.top, m.title.top),
    right: Math.max(m.mark.right, m.title.right),
    bottom: Math.max(m.mark.bottom, m.title.bottom),
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
  const layout = {
    left: Math.min(m.markLayout.left, m.title.left),
    top: Math.min(m.markLayout.top, m.title.top),
    right: Math.max(m.markLayout.right, m.title.right),
    bottom: Math.max(m.markLayout.bottom, m.title.bottom),
  }
  if (!m.markLayoutOwned)
    failures.push("the fixed message grid does not own the mark's layout")
  const markCentre = {
    dx: (m.mark.left + m.mark.right - m.markLayout.left - m.markLayout.right) / 2,
    dy: (m.mark.top + m.mark.bottom - m.markLayout.top - m.markLayout.bottom) / 2,
  }
  if (Math.abs(markCentre.dx) > 1 || Math.abs(markCentre.dy) > 1)
    failures.push(
      `the painted mark left its layout centre by ${markCentre.dx.toFixed(1)}, ${markCentre.dy.toFixed(1)}`,
    )
  if (m.phase !== null && !m.markAnimationPresent)
    failures.push("the loading mark has no breathing animation to sample")
  if (m.phase !== null && m.phaseMilliseconds === null)
    failures.push("the loading mark has no numeric animation duration to sample")
  if (m.phase !== null && !m.keyframeOffsets?.includes(m.phase))
    failures.push(`the loading mark has no breathing keyframe at phase ${m.phase}`)
  if (scenario.centred) {
    const dx = (layout.left + layout.right) / 2 - (clip.left + clip.right) / 2
    const dy = (layout.top + layout.bottom) / 2 - (clip.top + clip.bottom) / 2
    if (Math.abs(dx) > 1 || Math.abs(dy) > 1)
      failures.push(
        `off centre of the visible window by ${dx.toFixed(1)}, ${dy.toFixed(1)}`,
      )
  }
  if (scenario.host === null) {
    if (!m.text.includes("did not serve"))
      failures.push(`says ${JSON.stringify(m.text)}, not that the script was not served`)
  } else if (m.text !== "Loading")
    failures.push(`says ${JSON.stringify(m.text)}, not "Loading"`)
  if (!m.titleOnTop) failures.push("something paints over the title")
  if (m.overflow) failures.push("the page scrolls")
  return { failures, measured: { clip, content, layout, markCentre } }
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
                  // Without a host the frontend mounts at once, so hold back every
                  // script it loads (dev's /src/main.tsx or a build's hashed entry).
                  const holding = scenario.host === null
                  await page.route("**/*", (route) => {
                    if (!holding || route.request().resourceType() !== "script")
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
                  if (typeof scenario.host === "object" && scenario.host)
                    await page.waitForFunction(() =>
                      document.documentElement.style.getPropertyValue(
                        "--nessa-window-width",
                      ),
                    )
                  else if (scenario.host === null)
                    await page.waitForFunction(() =>
                      document
                        .querySelector("[data-nessa-load-title]")
                        ?.textContent?.includes("did not serve"),
                    )
                  else await page.waitForLoadState("networkidle")
                  await page.evaluate(() => new Promise(requestAnimationFrame))
                  // Sample the full-size and midpoint breathing keyframes;
                  // containment must not depend on when the page becomes ready. Only the mark's animation
                  // is paused; other fallback animations retain their behavior.
                  const samples = []
                  for (const phase of motion === "reduce" ? [null] : [0, 0.5]) {
                    const m = await measure(page, phase)
                    samples.push({
                      phase,
                      milliseconds: m.phaseMilliseconds,
                      keyframeOffsets: m.keyframeOffsets,
                      ...check(scenario, m),
                      animations: m.animations,
                    })
                  }
                  const result = {
                    failures: samples.flatMap((sample) =>
                      sample.failures.map((failure) =>
                        sample.phase === null
                          ? failure
                          : `at phase ${sample.phase} (${sample.milliseconds}ms): ${failure}`,
                      ),
                    ),
                    measured: samples[0].measured,
                    samples,
                  }
                  if (holding && held.size === 0)
                    result.failures.push("no frontend script was held back")
                  if (!(await page.$(css.loadMark)))
                    result.failures.push("the frontend replaced the fallback")
                  result.failures.push(...errors)
                  if (motion === "reduce" && samples[0].animations !== 0)
                    result.failures.push(
                      `${samples[0].animations} animations run with reduced motion`,
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
