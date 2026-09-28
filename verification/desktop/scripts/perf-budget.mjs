#!/usr/bin/env node
/**
 * The performance budget (ADR 238, Context — "Calm means no dropped
 * frames"): no frame over 50 ms on split, move, close, sidebar and list
 * toggles, send, typing, streaming, a pane dragged across zones, or opening
 * and answering in the Agents overview, in a production build at 4× CPU
 * throttling.
 *
 * Every interaction runs in a fresh page, N times per layout. Each run also
 * checks that the interaction did what it is named for (a split adds a pane,
 * a move reorders), so a no-op cannot pass as fast.
 */
import { attempt, CannotRun, log, table } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import {
  budgetMs,
  calibrate,
  calibrationFrame,
  measure,
  median,
  observers,
  throttle,
} from "./lib/perf.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys } from "./lib/selectors.mjs"
import { focusComposer, openPanes, order, paneCount, state } from "./lib/workspace.mjs"

const snapshot = async (page) => ({
  panes: await paneCount(page),
  order: (await order(page)).join(","),
  ...(await state(page)),
  requests: await page.locator(css.overviewRequest).count(),
  ghost: await page.locator(css.dragGhost).count(),
})

async function dragAcross(page, { cancel }) {
  const handle = page.locator(css.paneDragHandle).first()
  const box = await handle.boundingBox()
  const grid = await page.locator(css.paneGrid).first().boundingBox()
  if (!box || !grid)
    throw new CannotRun(
      `no drag handle (${css.paneDragHandle}) or grid (${css.paneGrid})`,
    )
  await page.mouse.move(box.x + Math.min(40, box.width / 3), box.y + box.height / 2)
  await page.mouse.down()
  const points = cancel
    ? [[0.9, 0.75]]
    : [
        [0.9, 0.25],
        [0.75, 0.75],
        [0.25, 0.9],
        [0.1, 0.6],
        [0.6, 0.1],
        [0.95, 0.5],
      ]
  for (const [fx, fy] of points) {
    await page.mouse.move(grid.x + grid.width * fx, grid.y + grid.height * fy, {
      steps: 12,
    })
    await page.waitForTimeout(250)
  }
  if (cancel) await page.keyboard.press("Escape")
  await page.mouse.up()
}

async function openOverview(page) {
  await focusComposer(page)
  await page.keyboard.press(keys.overview)
  await page.waitForTimeout(900)
  if ((await state(page)).content !== content.overview)
    throw new CannotRun(
      `⌘0 did not show the Agents overview (data-content ≠ ${content.overview})`,
    )
}

/**
 * Each budgeted interaction: `setup` (unmeasured), `act` (measured),
 * `settle` (ms measured after `act`), `expect(before, after)` (a failure
 * message when the interaction did not do what it is named for).
 */
const scenarios = {
  "split-right": {
    act: (p) => p.keyboard.press(keys.newSessionBeside),
    expect: (b, a) =>
      a.panes === b.panes + 1 ? null : `panes ${b.panes} → ${a.panes}, expected +1`,
  },
  "split-down": {
    act: (p) => p.keyboard.press(keys.splitDown),
    expect: (b, a) =>
      a.panes === b.panes + 1 ? null : `panes ${b.panes} → ${a.panes}, expected +1`,
  },
  move: {
    setup: (p) => openPanes(p, 2),
    act: (p) => p.keyboard.press(keys.moveLeft),
    expect: (b, a) => (a.order !== b.order ? null : `order unchanged (${b.order})`),
  },
  close: {
    setup: (p) => openPanes(p, 2),
    act: (p) => p.keyboard.press(keys.closePane),
    expect: (b, a) =>
      a.panes === b.panes - 1 ? null : `panes ${b.panes} → ${a.panes}, expected −1`,
  },
  sidebar: { act: (p) => p.keyboard.press(keys.toggleSidebar) },
  "session-list": {
    layouts: ["columns"],
    act: (p) => p.keyboard.press(keys.toggleSessionList),
  },
  "send-home": {
    setup: async (p) => {
      await p.keyboard.press(keys.newSession)
      await p.waitForTimeout(700)
      await focusComposer(p)
      await p.keyboard.type("hello from home", { delay: 20 })
    },
    act: (p) => p.keyboard.press("Enter"),
    settle: 1500,
  },
  typing: {
    setup: (p) => focusComposer(p),
    act: (p) => p.keyboard.type("the quick brown fox jumps over", { delay: 60 }),
  },
  "stream-1-of-4": {
    setup: async (p) => {
      await openPanes(p, 4)
      await focusComposer(p)
      await p.keyboard.type("go", { delay: 20 })
    },
    act: (p) => p.keyboard.press("Enter"),
    settle: 6000,
  },
  "drag-drop": {
    setup: (p) => openPanes(p, 4),
    act: (p) => dragAcross(p, { cancel: false }),
    settle: 1200,
    expect: (b, a) =>
      a.ghost === 0 ? null : "the carried copy is still on the page after the drop",
  },
  "drag-cancel": {
    setup: (p) => openPanes(p, 4),
    act: (p) => dragAcross(p, { cancel: true }),
    settle: 1200,
    expect: (b, a) =>
      a.order === b.order && a.ghost === 0
        ? null
        : `cancel changed the layout (${b.order} → ${a.order}) or left the copy`,
  },
  "overview-open": {
    setup: (p) => focusComposer(p),
    act: (p) => p.keyboard.press(keys.overview),
    expect: (b, a) =>
      a.content === content.overview
        ? null
        : `content ${a.content}, expected ${content.overview}`,
  },
  "overview-select": {
    setup: openOverview,
    act: async (p) => {
      for (let i = 0; i < 3; i++) {
        await p.keyboard.press("ArrowDown")
        await p.waitForTimeout(200)
      }
    },
    expect: (b, a) =>
      a.activeOverviewItem && a.activeOverviewItem !== b.activeOverviewItem
        ? null
        : "the selection did not move",
  },
  "overview-answer": {
    setup: async (p) => {
      await openOverview(p)
      await p.keyboard.press("Home")
      await p.waitForTimeout(300)
    },
    act: (p) => p.keyboard.press(keys.allow),
    settle: 1500,
    expect: (b, a) =>
      a.requests < b.requests
        ? null
        : `requests ${b.requests} → ${a.requests}, expected fewer`,
  },
  "overview-leave": {
    setup: openOverview,
    act: (p) => p.keyboard.press("Escape"),
    expect: (b, a) =>
      a.content === content.panes
        ? null
        : `content ${a.content}, expected ${content.panes}`,
  },
}

const meta = {
  name: "perf-budget",
  summary: `no frame over ${budgetMs} ms for every budgeted interaction`,
  defaults: { mode: "prod", engine: "chromium" },
  options: {
    runs: { type: "string", default: "3" },
    throttle: { type: "string", default: "4" },
    only: { type: "string" },
    width: { type: "string", default: "1600" },
    height: { type: "string", default: "1000" },
  },
  help: `
Usage: node verification/desktop/scripts/perf-budget.mjs [options]

  --runs <n>          Fresh-page runs per interaction and layout (default 3).
  --throttle <rate>   CDP CPU throttling rate (default 4, the budget's).
  --only <list>       Interactions to run, comma-separated. Available:
                      ${Object.keys(scenarios).join(", ")}
  --width, --height   Viewport (default 1600 × 1000).

Defaults to --mode prod (the budget is stated for a production build) and
--engine chromium (throttling and Long Animation Frame timing are Chromium's).
Before measuring, calibration confirms the measurement works: a busy loop
run unthrottled and throttled (the ratio must approach the rate), and one
frame of known cost (120 ms) must be measured at least that long and
attributed by a Long Animation Frame.

The table on stderr: max and median of each run's longest frame, and how
many frames over ${budgetMs} ms across runs. The JSON (stdout or --out) keeps,
for every over-budget frame, the Long Animation Frame that covers it:
blocking time, style-and-layout time, and its longest scripts with their
forced layout.`,
}

await main(meta, async ({ options, rep, url, mode }) => {
  if (options.engines.some((e) => e !== "chromium"))
    throw new CannotRun(
      "perf-budget measures in Chromium only (CDP throttling, Long Animation Frames)",
    )
  if (mode === "dev")
    log("note: --mode dev — StrictMode and unminified code; numbers are not the budget's")
  const runs = Number(options.runs)
  const rate = Number(options.throttle)
  const only = options.only ? options.list(options.only) : null
  const unknown = (only ?? []).filter((n) => !scenarios[n])
  if (unknown.length) throw new CannotRun(`unknown interaction(s): ${unknown.join(", ")}`)
  const viewport = { width: Number(options.width), height: Number(options.height) }
  const rows = []

  await withEngines(options, rep, async (engine, browser) => {
    const calibration = await attempt(rep, { name: "calibration", engine }, async () => {
      const { context, page, close } = await openPage(browser, {
        url,
        ...viewport,
        initScripts: [observers],
      })
      const result = await calibrate(context, page, rate)
      const frame = await calibrationFrame(page)
      await close()
      log(
        `calibration: busy loop ${result.plainMs} ms → ${result.throttledMs} ms at ${rate}× (ratio ${result.ratio})`,
      )
      log(
        `calibration: a ${frame.cost} ms frame measured ${frame.measuredMs} ms, attributed by LoAF: ${frame.attributed}`,
      )
      const failures = []
      if (!result.ok)
        failures.push(`throttle did not apply: ratio ${result.ratio} at ${rate}×`)
      if (!frame.ok)
        failures.push(
          `a ${frame.cost} ms frame measured ${frame.measuredMs} ms (attributed: ${frame.attributed})`,
        )
      return { calibration: result, calibrationFrame: frame, failures }
    })
    if (!calibration.ok) return

    for (const layout of options.layouts)
      for (const [name, s] of Object.entries(scenarios)) {
        if (only && !only.includes(name)) continue
        if (s.layouts && !s.layouts.includes(layout)) continue
        await attempt(rep, { name, engine, layout }, async () => {
          const detail = []
          for (let r = 0; r < runs; r++) {
            const opened = await openPage(browser, {
              url,
              layout,
              ...viewport,
              initScripts: [observers],
            })
            try {
              const { context, page } = opened
              await need(page, css.pane, "a pane")
              await s.setup?.(page, layout)
              await page.waitForTimeout(800)
              await throttle(context, page, rate)
              await page.waitForTimeout(400)
              const before = await snapshot(page)
              const m = await measure(page, () => s.act(page), s.settle ?? 1000)
              const after = await snapshot(page)
              detail.push({
                ...m,
                did: s.expect?.(before, after) ?? null,
                errors: opened.errors,
              })
            } finally {
              await opened.close()
            }
          }
          const maxes = detail.map((d) => d.maxFrame)
          const over = detail.reduce((n, d) => n + d.over, 0)
          const row = {
            layout,
            name,
            max: Math.max(...maxes),
            median: median(maxes),
            over50: over,
            runs: maxes.join(" "),
          }
          rows.push(row)
          const failures = []
          if (row.max > budgetMs)
            failures.push(
              `longest frame ${row.max} ms > ${budgetMs} ms (runs: ${row.runs})`,
            )
          for (const d of detail)
            if (d.did) failures.push(`did not do what it is named for: ${d.did}`)
          for (const d of detail) for (const e of d.errors) failures.push(e)
          if (detail.some((d) => d.noLoaf))
            log(
              "note: no Long Animation Frame timing in this browser; attribution is empty",
            )
          return {
            max: row.max,
            median: row.median,
            over50: over,
            runs: detail,
            failures,
          }
        })
      }
  })
  if (rows.length)
    log(
      `\n${table(rows, ["layout", "name", "max", "median", "over50", "runs"])}\n(ms; budget ${budgetMs} ms at ${rate}× throttle)\n`,
    )
})
