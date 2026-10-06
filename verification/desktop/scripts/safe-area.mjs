#!/usr/bin/env node
/**
 * The titlebar's safe area (ADR 238, "The titlebar's safe area"): in every
 * frame of every transition that moves titlebar content, nothing is painted
 * under the window's controls — text, icons, images, code, decorative art —
 * in Chrome and WebKit, at 1440 × 900 and 1000 × 700.
 */
import { attempt, chosen } from "./lib/cli.mjs"
import { launch, need, openPage } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { safeArea, safeAreaInit, summarize } from "./lib/safe-area.mjs"
import { css, keys, storage } from "./lib/selectors.mjs"
import { hideColumns, leaveSettings, settled, switchLayout } from "./lib/workspace.mjs"

/**
 * Each scenario gets `{ page, watch, layout, size }` and drives the
 * transitions it names through `watch(step, act, ms)`.
 */
const scenarios = {
  "sidebar-toggle": async ({ page, watch }) => {
    await watch("⌘B hide", () => page.keyboard.press(keys.toggleSidebar))
    await watch("⌘B show", () => page.keyboard.press(keys.toggleSidebar))
  },
  "session-list-toggle": async ({ page, watch, layout }) => {
    if (layout !== "columns") return "skipped: no session list in this layout"
    await watch("⌥⌘S hide", () => page.keyboard.press(keys.toggleSessionList))
    await watch("⌥⌘S show", () => page.keyboard.press(keys.toggleSessionList))
    await page.keyboard.press(keys.toggleSidebar)
    await watch("⌥⌘S hide, sidebar hidden", () =>
      page.keyboard.press(keys.toggleSessionList),
    )
    await watch("⌥⌘S show, sidebar hidden", () =>
      page.keyboard.press(keys.toggleSessionList),
    )
  },
  "edge-peek": async ({ page, watch, size }) => {
    await page.keyboard.press(keys.toggleSidebar)
    await settled(page)
    await watch("peek in", () => page.mouse.move(3, size.height / 2))
    await watch("peek out", () => page.mouse.move(size.width - 100, size.height / 2))
  },
  "drag-to-collapse": async ({ page, watch }) => {
    const edge = page.locator(css.sidebarEdge).first()
    await need(page, css.sidebarEdge, "the sidebar's resize edge")
    const box = await edge.boundingBox()
    const y = box.y + box.height / 2
    await page.mouse.move(box.x + box.width / 2, y)
    await page.mouse.down()
    await watch("drag past narrowest", () =>
      page.mouse.move(box.x - 260, y, { steps: 20 }),
    )
    await watch("drag back out", () => page.mouse.move(box.x + 40, y, { steps: 20 }))
    await page.mouse.up()
  },
  "resize-breakpoints": async ({ page, watch, size }) => {
    for (let w = size.width; w >= 560; w -= 60)
      await watch(
        `narrow to ${w}`,
        () => page.setViewportSize({ width: w, height: size.height }),
        350,
      )
    for (let w = 560; w <= size.width; w += 60)
      await watch(
        `widen to ${w}`,
        () => page.setViewportSize({ width: w, height: size.height }),
        350,
      )
  },
  settings: async ({ page, watch, size }) => {
    await watch("open Settings", () => page.keyboard.press(keys.settings))
    await watch("Settings ⌘B hide", () => page.keyboard.press(keys.toggleSidebar))
    await watch("Settings peek in", () => page.mouse.move(3, size.height / 2))
    await watch("Settings peek out", () =>
      page.mouse.move(size.width - 100, size.height / 2),
    )
    await watch("Settings ⌘B show", () => page.keyboard.press(keys.toggleSidebar))
    await watch("leave Settings", () => leaveSettings(page))
  },
  "settings-sidebar-return": async ({ page, watch }) => {
    await page.keyboard.press(keys.toggleSidebar)
    await settled(page)
    await watch("open Settings, sidebar hidden", () => page.keyboard.press(keys.settings))
    await watch("leave Settings, sidebar hidden", () => leaveSettings(page))
  },
  "layout-switch": async ({ page, watch, layout }) => {
    const other = layout === "columns" ? "sidebar" : "columns"
    await watch(`switch to ${other}`, () => switchLayout(page, other), 1000)
    await watch(`switch back to ${layout}`, () => switchLayout(page, layout), 1000)
  },
  "split-and-close": async ({ page, watch }) => {
    await watch("⇧⌘N", () => page.keyboard.press(keys.newSessionBeside))
    await watch("⇧⌘\\", () => page.keyboard.press(keys.splitDown))
    await watch("⌘W", () => page.keyboard.press(keys.closePane))
    await watch("⌘W", () => page.keyboard.press(keys.closePane))
  },
  "scrolled-transcript": async ({ page, watch, layout }) => {
    await hideColumns(page, layout)
    await need(page, css.transcript, "a transcript")
    for (const y of [0, 60, 120, 200, 400])
      await watch(
        `scrollTop ${y}`,
        () =>
          page.evaluate(
            ([sel, y]) => (document.querySelector(sel).scrollTop = y),
            [css.transcript, y],
          ),
        150,
      )
  },
  overview: async ({ page, watch }) => {
    await page.locator(css.composer).first().click()
    await watch("⌘0 open", () => page.keyboard.press(keys.overview), 1000)
    await watch("⌘B in overview", () => page.keyboard.press(keys.toggleSidebar))
    await watch("⌘B in overview", () => page.keyboard.press(keys.toggleSidebar))
    await watch("Esc leave", () => page.keyboard.press(keys.escape), 1000)
  },
}

const meta = {
  name: "safe-area",
  summary: "nothing is painted under the window controls, in any frame",
  defaults: { engine: "chromium,webkit" },
  options: {
    only: { type: "string" },
    jobs: { type: "string", default: "4" },
    sizes: { type: "string", default: "1440x900,1000x700" },
    "reduced-motion": { type: "boolean", default: false },
    picture: { type: "boolean", default: true },
  },
  help: `
Usage: node verification/desktop/scripts/safe-area.mjs [options]

  --only <list>        Scenarios, comma-separated. Available:
                       ${Object.keys(scenarios).join(", ")}
  --sizes <list>       Window sizes (default 1440x900,1000x700, the ADR's).
  --jobs <n>           Scenarios run at once, each worker with its own page per
                       engine, layout and size, reloaded between scenarios
                       (default 4).
  --reduced-motion     Emulate prefers-reduced-motion: reduce.
  --no-picture         Turn the picture band in conversations off (on by default,
                       so the band is checked under the controls).

Every frame of each step is sampled: any text, icon, image, input or button
whose painted box (clipped by its ancestors) enters x < --desktop-titlebar-safe-start,
y < --desktop-titlebar-height is a violation. Titlebar rows, tooltips, the
drag copy and the hidden title sizer are exempt (lib/selectors.mjs,
safeAreaExempt); decorative art — the picture band, the night scene — is not.
A positive control first places a label at (8, 8) and must see it caught.
A step ends once its motion has (nothing finite animating, at least 250ms
after its act), at most its own limit.
The page is marked as the macOS window so the safe area includes the traffic lights.`,
}

/** Runs `work` over `items`, at most `jobs` at once. */
async function pool(items, jobs, work) {
  const queue = [...items]
  const runners = Array.from({ length: Math.min(jobs, queue.length) }, async () => {
    while (queue.length) await work(queue.shift())
  })
  await Promise.all(runners)
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only
    ? chosen(options.only, Object.keys(scenarios), options.list)
    : null
  const sizes = options.choices("sizes").map((s) => {
    const [width, height] = s.split("x").map(Number)
    return { width, height }
  })
  const jobs = Math.max(1, Number(options.jobs) || 1)
  // One browser per engine, shared by its groups; one that does not launch is could-not-run.
  const browsers = new Map()
  try {
    for (const engine of options.engines) {
      try {
        browsers.set(engine, await launch(engine, options))
      } catch (error) {
        rep.add({ name: "launch", engine, cannotRun: true, error: error.message })
      }
    }
    // Positive control, per engine: a label placed under the controls must be
    // caught, or a clean result below means nothing.
    const trusted = new Set()
    await pool([...browsers], jobs, async ([engine, browser]) => {
      const control = await attempt(
        rep,
        { name: "sampler-control", engine },
        async () => {
          const opened = await openPage(browser, {
            url,
            layout: options.layouts[0],
            initScripts: [safeAreaInit],
          })
          try {
            const sampler = safeArea(opened.page)
            await sampler.take()
            await sampler.watch("control label under the controls", () =>
              opened.page.evaluate(() => {
                const probe = document.createElement("div")
                probe.textContent = "verify-control"
                probe.style.cssText =
                  "position:fixed;left:8px;top:8px;z-index:99999;font:12px sans-serif"
                document.body.append(probe)
                setTimeout(() => probe.remove(), 300)
              }),
            )
            const bad = await sampler.take()
            const caught = bad.filter((b) => b.label === "verify-control")
            return {
              safe: caught[0]?.safe,
              bar: caught[0]?.bar,
              failures: caught.length
                ? []
                : ["the sampler did not catch a label placed at (8, 8)"],
            }
          } finally {
            await opened.close()
          }
        },
      )
      if (control.ok) trusted.add(engine)
    })
    const groups = [...browsers]
      .filter(([engine]) => trusted.has(engine))
      .flatMap(([engine, browser]) =>
        options.layouts.flatMap((layout) =>
          sizes.map((size) => ({ engine, browser, layout, size })),
        ),
      )
    const wanted = Object.entries(scenarios).filter(
      ([name]) => !only || only.includes(name),
    )
    // Every scenario of every group is a unit of work; each worker keeps one
    // page per group and loads it fresh between scenarios — the window's
    // state, size and stored layout put back — so a page is opened once per
    // worker and group, not once per scenario.
    const units = groups.flatMap((group) =>
      wanted.map(([name, scenario]) => ({ ...group, name, scenario })),
    )
    const pageFor = (group) =>
      openPage(group.browser, {
        url,
        layout: group.layout,
        ...group.size,
        reducedMotion: options["reduced-motion"] ? "reduce" : undefined,
        prefs: {
          [storage.pictureInConversations]: options.picture ? "on" : "off",
        },
        initScripts: [safeAreaInit],
      })
    const keyOf = ({ engine, layout, size }) =>
      `${engine} ${layout} ${size.width}x${size.height}`
    const workers = Array.from({ length: Math.min(jobs, units.length) }, async () => {
      let held = null
      try {
        while (units.length) {
          const unit = units.shift()
          const { engine, layout, size, name, scenario } = unit
          await attempt(
            rep,
            { name, engine, layout, width: `${size.width}x${size.height}` },
            async () => {
              if (held?.key !== keyOf(unit)) {
                await held?.opened.close()
                held = null
                held = { key: keyOf(unit), opened: await pageFor(unit), fresh: true }
              }
              const { opened } = held
              const { page } = opened
              if (!held.fresh) {
                await page.mouse.up().catch(() => {})
                await page.setViewportSize(size)
                await page.evaluate(
                  ([key, value]) => localStorage.setItem(key, value),
                  [storage.layout, layout],
                )
                await page.reload({ waitUntil: "domcontentloaded" })
                await need(page, css.anyReady, "the desktop page", 30_000)
                await settled(page)
                opened.noteHeldHarmless()
              }
              held.fresh = false
              const sampler = safeArea(page)
              await sampler.take()
              const skipped = await scenario({ page, watch: sampler.watch, layout, size })
              if (typeof skipped === "string") return { note: skipped }
              const bad = await sampler.take()
              const frames = await sampler.frames()
              const failures = summarize(bad)
              if (frames === 0) failures.push("no frames were sampled")
              return { frames, violations: bad.length, failures }
            },
          )
        }
      } finally {
        await held?.opened.close()
      }
    })
    await Promise.all(workers)
  } finally {
    for (const browser of browsers.values()) await browser.close()
  }
})
