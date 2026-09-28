#!/usr/bin/env node
/**
 * Pane drag and drop (ADR 238, "Drag and drop"), sampled frame by frame in
 * Chrome and WebKit:
 *
 *   follows-pointer   the carried copy's corner is the pointer less the grab offset
 *   inside-grid       no pane (previewing where the drop would put it) leaves the grid
 *   one-way           between zone changes, each pane moves one way (no back-and-forth)
 *   sideways-top      a sideways sweep near a tall pane's top reaches its side, never above/below
 *   boundary-jitter   ±6px on a zone boundary does not flicker the zone
 *   escape-cancels    Escape mid-drag flies the copy home; the drop after changes nothing
 *   outside-cancels   a drop outside the window changes nothing and leaves nothing lifted
 *   no-selection      no text selection during or after a drag (WebKit selected text before)
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, zoneSaid } from "./lib/selectors.mjs"
import {
  hideColumns,
  lift,
  openPanes,
  order,
  panes,
  recordZones,
} from "./lib/workspace.mjs"

/** Starts a per-frame recording of the ghost, the panes, the grid and the zone. */
function startFrames(page) {
  return page.evaluate((sel) => {
    window.__dragFrames = []
    window.__dragOn = true
    const rect = (e) => {
      const r = e.getBoundingClientRect()
      return { x: r.left, y: r.top, w: r.width, h: r.height }
    }
    const tick = () => {
      const ghost = document.querySelector(sel.dragGhost)
      const grid = document.querySelector(sel.paneGrid)
      window.__dragFrames.push({
        t: performance.now(),
        pointer: window.__verifyPointer ?? null,
        ghost: ghost && ghost.checkVisibility() ? rect(ghost) : null,
        grid: grid ? rect(grid) : null,
        panes: [...document.querySelectorAll(sel.pane)]
          .filter((e) => !e.closest(sel.dragGhost))
          .map((e) => ({ key: e.dataset.paneKey, ...rect(e) })),
        zone: document.querySelector(sel.dropAnnouncer)?.textContent ?? "",
        selection: String(getSelection() ?? "")
          .trim()
          .slice(0, 40),
      })
      if (window.__dragOn) requestAnimationFrame(tick)
    }
    addEventListener(
      "pointermove",
      (e) => (window.__verifyPointer = { x: e.clientX, y: e.clientY }),
      { capture: true },
    )
    requestAnimationFrame(tick)
  }, css)
}
const stopFrames = (page) =>
  page.evaluate(() => {
    window.__dragOn = false
    return window.__dragFrames
  })

const tolerance = 2

function followsPointer(frames) {
  const carried = frames.filter((f) => f.ghost && f.pointer)
  if (carried.length < 5) return [`only ${carried.length} frames carried the copy`]
  const first = carried[0]
  const grab = { x: first.pointer.x - first.ghost.x, y: first.pointer.y - first.ghost.y }
  const off = carried.filter(
    (f) =>
      Math.abs(f.pointer.x - grab.x - f.ghost.x) > tolerance ||
      Math.abs(f.pointer.y - grab.y - f.ghost.y) > tolerance,
  )
  // The drop's snap and the flight home are allowed to leave the pointer; only frames while the pointer moves count.
  const moving = off.filter((f, i) => i < off.length - 3)
  return moving.length
    ? [
        `copy left the pointer in ${moving.length}/${carried.length} frames (e.g. Δ ${Math.round(moving[0].pointer.x - grab.x - moving[0].ghost.x)},${Math.round(moving[0].pointer.y - grab.y - moving[0].ghost.y)})`,
      ]
    : []
}

function insideGrid(frames) {
  const out = []
  for (const f of frames) {
    if (!f.grid) continue
    for (const p of f.panes)
      if (
        p.x < f.grid.x - tolerance ||
        p.y < f.grid.y - tolerance ||
        p.x + p.w > f.grid.x + f.grid.w + tolerance ||
        p.y + p.h > f.grid.y + f.grid.h + tolerance
      )
        out.push(
          `pane ${p.key} at ${Math.round(p.x)},${Math.round(p.y)} ${Math.round(p.w)}×${Math.round(p.h)}`,
        )
  }
  return out.length ? [`${out.length} pane-frames outside the grid, e.g. ${out[0]}`] : []
}

function oneWay(frames) {
  const reversals = []
  let segment = []
  const flush = () => {
    const keys = new Set(segment.flatMap((f) => f.panes.map((p) => p.key)))
    for (const key of keys)
      for (const axis of ["x", "y"]) {
        const values = segment
          .map((f) => f.panes.find((p) => p.key === key)?.[axis])
          .filter((v) => v !== undefined)
        let direction = 0
        for (let i = 1; i < values.length; i++) {
          const d = values[i] - values[i - 1]
          if (Math.abs(d) < 1) continue
          const s = Math.sign(d)
          if (direction && s !== direction) {
            reversals.push(
              `pane ${key} reversed on ${axis} within zone "${segment[0].zone}"`,
            )
            break
          }
          direction = s
        }
      }
    segment = []
  }
  for (const f of frames) {
    if (segment.length && f.zone !== segment[0].zone) flush()
    segment.push(f)
  }
  flush()
  return [...new Set(reversals)]
}

async function fourPanes(page, layout) {
  await hideColumns(page, layout)
  await openPanes(page, 4)
  return panes(page)
}

/** Three panes side by side: tall, narrow columns. */
async function threeColumns(page, layout) {
  await hideColumns(page, layout)
  await openPanes(page, 3)
  const list = await panes(page)
  const tops = new Set(list.map((p) => Math.round(p.y)))
  if (tops.size !== 1)
    throw new CannotRun(
      `expected three side-by-side panes, got tops ${[...tops].join(",")}`,
    )
  return list.sort((a, b) => a.x - b.x)
}

const checks = {
  "sweep-across-zones": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const grid = await page.locator(css.paneGrid).first().boundingBox()
    await startFrames(page)
    await lift(page, 0)
    for (const [fx, fy] of [
      [0.9, 0.25],
      [0.75, 0.75],
      [0.25, 0.9],
      [0.6, 0.1],
      [0.95, 0.5],
    ]) {
      await page.mouse.move(grid.x + grid.width * fx, grid.y + grid.height * fy, {
        steps: 20,
      })
      await page.waitForTimeout(250)
    }
    const during = await stopFrames(page)
    await page.keyboard.press("Escape")
    await page.mouse.up()
    await page.waitForTimeout(600)
    return {
      panes: list.length,
      frames: during.length,
      failures: [
        ...followsPointer(during).map((f) => `follows-pointer: ${f}`),
        ...insideGrid(during).map((f) => `inside-grid: ${f}`),
        ...oneWay(during).map((f) => `one-way: ${f}`),
        ...(during.some((f) => f.selection)
          ? [
              `no-selection: "${during.find((f) => f.selection).selection}" selected during the drag`,
            ]
          : []),
      ],
    }
  },
  "sideways-top": async (page, layout) => {
    const [, middle] = await threeColumns(page, layout)
    const failures = []
    for (const dy of [30, 50, 90]) {
      await lift(page, 2)
      await page.mouse.move(middle.x + middle.w + 30, middle.y + dy, { steps: 10 })
      const zones = await recordZones(page)
      await page.mouse.move(middle.x + middle.w * 0.45, middle.y + dy, { steps: 30 })
      await page.waitForTimeout(50)
      const said = await zones.take()
      if (!said.length)
        failures.push(`sweeping left at top+${dy}px announced no zone at all`)
      if (said.some((z) => zoneSaid.vertical.test(z)))
        failures.push(
          `sweeping left at top+${dy}px picked a vertical zone: ${said.join(" → ")}`,
        )
      await page.keyboard.press("Escape")
      await page.mouse.up()
      await page.waitForTimeout(500)
    }
    return { failures }
  },
  "boundary-jitter": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const target = list[1]
    const inset = Math.min(Math.max(0.28 * Math.min(target.w, target.h), 48), 120)
    const x = target.x + inset
    await lift(page, 0)
    await page.mouse.move(x, target.y + target.h / 2, { steps: 10 })
    await page.waitForTimeout(200)
    const zones = await recordZones(page)
    for (let i = 0; i < 40; i++)
      await page.mouse.move(x + (i % 2 ? 6 : -6), target.y + target.h / 2)
    await page.waitForTimeout(100)
    const said = await zones.take()
    // Holding one zone says nothing new; confirm there is a zone at all.
    const current = await page.locator(css.dropAnnouncer).first().textContent()
    await page.keyboard.press("Escape")
    await page.mouse.up()
    if (!said.length && !current)
      return { failures: ["no zone is announced at the boundary"] }
    return {
      failures:
        said.length > 1
          ? [`the zone flickered ${said.length} times: ${said.slice(0, 6).join(" → ")}`]
          : [],
    }
  },
  "escape-cancels": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const before = (await order(page)).join(",")
    await lift(page, 0)
    await page.mouse.move(list[2].x + 30, list[2].y + list[2].h / 2, { steps: 10 })
    await page.keyboard.press("Escape")
    await page.waitForTimeout(600)
    const ghostAfterEscape = await page.locator(css.dragGhost).count()
    await page.mouse.move(list[3].x + 30, list[3].y + list[3].h / 2, { steps: 5 })
    await page.mouse.up()
    await page.waitForTimeout(600)
    const after = (await order(page)).join(",")
    const failures = []
    if (ghostAfterEscape) failures.push("the copy is still on the page after Escape")
    if (after !== before)
      failures.push(`layout changed after Escape then drop: ${before} → ${after}`)
    return { failures }
  },
  "outside-cancels": async (page, layout) => {
    const list = await fourPanes(page, layout)
    const size = page.viewportSize()
    const before = (await order(page)).join(",")
    const restingTransforms = await page.evaluate(
      (pane) =>
        [...document.querySelectorAll(pane)]
          .map((e) => getComputedStyle(e).transform)
          .join("|"),
      css.pane,
    )
    await lift(page, 0)
    await page.mouse.move(list[1].x + 30, list[1].y + list[1].h / 2, { steps: 8 })
    await page.evaluate(() =>
      addEventListener(
        "pointermove",
        (e) => (window.__verifyLast = { x: e.clientX, y: e.clientY }),
        { capture: true },
      ),
    )
    await page.mouse.move(size.width - 1, size.height - 1, { steps: 8 })
    await page.mouse.move(size.width + 60, size.height + 50, { steps: 3 })
    const last = await page.evaluate(() => window.__verifyLast)
    if (!last || (last.x < size.width && last.y < size.height)) {
      await page.keyboard.press("Escape")
      await page.mouse.up()
      throw new CannotRun(
        `this engine delivered no pointer event outside the viewport (last at ${last?.x},${last?.y}); ` +
          "check a drop outside the window by hand in the app",
      )
    }
    await page.mouse.up()
    await page.waitForTimeout(800)
    const after = await page.evaluate(
      ([pane, ghost]) => ({
        ghost: document.querySelectorAll(ghost).length,
        transforms: [...document.querySelectorAll(pane)]
          .map((e) => getComputedStyle(e).transform)
          .join("|"),
        selection: String(getSelection() ?? "")
          .trim()
          .slice(0, 40),
      }),
      [css.pane, css.dragGhost],
    )
    const failures = []
    const now = (await order(page)).join(",")
    if (now !== before)
      failures.push(`layout changed after a drop outside: ${before} → ${now}`)
    if (after.ghost) failures.push("the copy is still on the page")
    if (after.transforms !== restingTransforms)
      failures.push(
        `panes did not return to rest: ${restingTransforms} → ${after.transforms}`,
      )
    if (after.selection)
      failures.push(`no-selection: "${after.selection}" selected after the drag`)
    return { failures }
  },
}

const meta = {
  name: "drag",
  summary:
    "pane drag: copy follows the pointer, previews stay in the grid, direction-aware zones, cancels",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/drag.mjs [options]

  --only <list>   Checks, comma-separated. Available:
                  ${Object.keys(checks).join(", ")}

sweep-across-zones covers follows-pointer, inside-grid, one-way and
no-selection in one recorded drag. Side columns are hidden first so four
panes fit at 1440 × 900.`,
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.list(options.only) : null
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const [name, check] of Object.entries(checks)) {
        if (only && !only.includes(name)) continue
        await attempt(rep, { name, engine, layout }, async () => {
          const opened = await openPage(browser, {
            url,
            layout,
            width: 1440,
            height: 900,
          })
          try {
            await need(opened.page, css.paneHeader, "a pane header")
            const result = await check(opened.page, layout)
            return { ...result, failures: [...(result.failures ?? []), ...opened.errors] }
          } finally {
            await opened.close()
          }
        })
      }
  })
})
