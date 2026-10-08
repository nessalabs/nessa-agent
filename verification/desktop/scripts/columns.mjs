#!/usr/bin/env node
/**
 * Columns: the window's columns never overlap, and the side rail's corner
 * holds still.
 *
 *   no-overlap       in each layout, at each size, with the side rail open and
 *                    closed, the sidebar and session list shown and hidden, and
 *                    the Agents overview open and closed, every drawn column —
 *                    the rail, the sidebar, the session list, then the chat
 *                    area or the overview — stands left to right, each
 *                    beginning where the one before it ends or later, none past
 *                    the window's edges. A column placed from the window's edge
 *                    without counting a column beside it (the overview that
 *                    slid under the sidebar once the rail opened) breaks it.
 *   rail-corner      the rail's toggle is at the same pixels in every state; it
 *                    is hidden exactly when the rail and the sidebar are both
 *                    closed; "nessa Studio" is at the same pixels in Agents and
 *                    in an item's full view, rail open and closed, and never
 *                    under the toggle.
 *   rail-view-inert  under an item's full view the workspace takes no keys:
 *                    ⌘B, ⌘0 and ⌥⌘S change nothing behind it, and coming back
 *                    to Agents finds it as it was left.
 *   titlebar-steady  every frame of a rail toggle draws the titlebar's controls
 *                    between where they were and where they land: still, in a
 *                    window whose controls are placed from its edge (macOS),
 *                    and never under its traffic lights.
 *   rail-off         the rail is a preview, off until Settings › Advanced ›
 *                    Experimental turns it on: off, there is no rail and no
 *                    toggle, nothing sits where the rail would, and "nessa
 *                    Studio" is in the sidebar's foot as without the rail.
 *
 * Every check but rail-off runs with the rail turned on.
 */
import { attempt } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, selectorFor, storage } from "./lib/selectors.mjs"
import { settled } from "./lib/workspace.mjs"

const checks = [
  "no-overlap",
  "rail-corner",
  "rail-view-inert",
  "titlebar-steady",
  "rail-off",
]

const meta = {
  name: "columns",
  summary: "no two columns overlap; the side rail's corner holds still",
  defaults: { engine: "chromium,webkit" },
  options: {
    only: { type: "string" },
    sizes: { type: "string", default: "1440x900,1000x700,760x700" },
  },
  help: `
Usage: node verification/desktop/scripts/columns.mjs [options]

  --only <list>    Checks, comma-separated: ${checks.join(", ")}
  --sizes <list>   Window sizes (default 1440x900,1000x700,760x700; --quick: the first).

Checks, per engine, layout and size:
  no-overlap       every combination of the rail, the sidebar, the session list
                   (columns layout) and the overview leaves the drawn columns in
                   order, none overlapping the next, none past the window's edges
  rail-corner      the toggle never moves and hides only with rail and sidebar
                   both closed; "nessa Studio" holds its place between Agents
                   and a full view and is never under the toggle
  rail-view-inert  ⌘B, ⌘0 and ⌥⌘S under a full view change nothing behind it
  titlebar-steady  through a rail toggle the titlebar's controls stay between
                   where they were and where they land, every frame
  rail-off         with the preview off (the default): no rail, no toggle`,
}

/** The columns in the order they stand; the chat area and the overview share the last place. */
const columnSelectors = [
  ["rail", css.sideRail],
  ["sidebar", css.sidebar],
  ["list", css.sessionList],
  ["chat", css.chat],
  ["overview", css.overview],
]

/** Each drawn column's left and right edge, in window pixels. */
function measure(columns) {
  const drawn = []
  for (const [name, selector] of columns) {
    const element = document.querySelector(selector)
    if (!element) continue
    const style = getComputedStyle(element)
    const box = element.getBoundingClientRect()
    if (style.display === "none" || style.visibility === "hidden") continue
    if (Number(style.opacity) === 0 || box.width < 2 || box.right <= 0) continue
    if (box.left >= window.innerWidth) continue
    drawn.push({ name, left: Math.round(box.left), right: Math.round(box.right) })
  }
  return { drawn, width: window.innerWidth }
}

/** What is wrong with one arrangement: an overlap, or a column past the window. */
function faults({ drawn, width }) {
  const failures = []
  const overview = drawn.some((c) => c.name === "overview")
  const ordered = drawn.filter((c) => c.name !== "chat" || !overview)
  for (let i = 1; i < ordered.length; i++) {
    const before = ordered[i - 1]
    const after = ordered[i]
    if (after.left < before.right - 1)
      failures.push(
        `${after.name} begins at ${after.left}, inside ${before.name} (ends ${before.right})`,
      )
  }
  for (const column of ordered)
    if (column.left < -1 || column.right > width + 1)
      failures.push(
        `${column.name} [${column.left}, ${column.right}] leaves the window (${width})`,
      )
  return failures
}

/** What is drawn, named as it is — not as it was asked for. */
const describe = ({ drawn }) => drawn.map((c) => c.name).join(" | ") || "nothing"

/**
 * Asks for the rail open or closed. Where the window has no room for it the
 * toggle says so (aria-disabled) or, with no column to sit in, is not drawn;
 * then nothing is asked, and the state measured says what was reached.
 */
async function setRail(page, open) {
  const toggle = page.locator(css.sideRailToggle)
  if (!(await toggle.isVisible())) return
  if ((await toggle.getAttribute("aria-disabled")) === "true") return
  if ((await expanded(page, css.sideRailToggle)) !== open) await toggle.click()
}

const expanded = (page, selector) =>
  page.evaluate(
    (s) => document.querySelector(s)?.getAttribute("aria-expanded") === "true",
    selector,
  )

/** A visible element's rounded box, or null when it is hidden or absent. */
const boxOf = (page, selector) =>
  page.evaluate((s) => {
    const element = [...document.querySelectorAll(s)].find((e) => {
      const style = getComputedStyle(e)
      return (
        e.getClientRects().length > 0 &&
        style.visibility !== "hidden" &&
        !e.closest("[inert]")
      )
    })
    if (!element) return null
    const b = element.getBoundingClientRect()
    return [Math.round(b.x), Math.round(b.y), Math.round(b.width), Math.round(b.height)]
  }, selector)

/** Waits out a change's motion, the pointer parked where it reveals nothing. */
async function rest(page, size) {
  await page.mouse.move(size.width - 40, size.height / 2)
  await settled(page)
  await page.waitForTimeout(450)
}

/** Sets the rail and the sidebar; the rail first, as its toggle hides with the sidebar. */
async function arrange(page, size, { rail, sidebar }) {
  if (!(await page.locator(css.sidebar).isVisible()))
    await page.keyboard.press(keys.toggleSidebar)
  await settled(page)
  await setRail(page, rail)
  if (!sidebar) await page.keyboard.press(keys.toggleSidebar)
  await rest(page, size)
}

const body = {
  "no-overlap": async ({ page, layout, size }) => {
    const failures = []
    const states = []
    const lists = layout === "columns" ? [true, false] : [true]
    for (const rail of [true, false])
      for (const list of lists)
        for (const sidebar of [true, false])
          for (const overview of [false, true]) {
            if (!(await page.locator(css.sidebar).isVisible()))
              await page.keyboard.press(keys.toggleSidebar)
            await settled(page)
            await setRail(page, rail)
            if (layout === "columns") {
              const shown = await page.locator(css.sessionList).isVisible()
              if (shown !== list) await page.keyboard.press(keys.toggleSessionList)
            }
            if (!sidebar) await page.keyboard.press(keys.toggleSidebar)
            if (overview) {
              // Focused, not clicked: at narrow sizes a click lands on what covers it.
              await page.locator(css.composer).first().focus()
              await page.keyboard.press(keys.overview)
              await need(page, css.overview, "the Agents overview")
            }
            await rest(page, size)
            const seen = await page.evaluate(measure, columnSelectors)
            const label = describe(seen)
            states.push({ label, drawn: seen.drawn })
            for (const f of faults(seen)) failures.push(`${label}: ${f}`)
            if (overview) {
              await page.keyboard.press(keys.escape)
              await settled(page)
            }
          }
    return { states: states.length, failures, detail: states }
  },

  "rail-corner": async ({ page, size }) => {
    const failures = []
    const seen = []
    let toggle = null
    const reached = () =>
      page.evaluate((frame) => {
        const window = document.querySelector(frame)
        return {
          rail: window?.dataset.rail === "open",
          sidebar: window?.dataset.sidebar === "open",
        }
      }, css.workspaceWindow)
    for (const rail of [true, false])
      for (const sidebar of [true, false]) {
        await arrange(page, size, { rail, sidebar })
        // Named as reached: where the window has no room the rail stays closed.
        const now = await reached()
        const label = `rail ${now.rail ? "open" : "closed"}, sidebar ${now.sidebar ? "shown" : "hidden"}`
        const at = await boxOf(page, css.sideRailToggle)
        const studio = await boxOf(page, css.studio)
        seen.push({ label, toggle: at, studio })
        const hidden = !now.rail && !now.sidebar
        if (hidden && at)
          failures.push(`${label}: the toggle is drawn with nothing to sit in`)
        if (!hidden && !at) failures.push(`${label}: the toggle is not drawn`)
        if (!at) continue
        toggle ??= at
        if (at.join() !== toggle.join())
          failures.push(`${label}: the toggle is at [${at}], not [${toggle}]`)
        if (studio && studio[0] < at[0] + at[2])
          failures.push(`${label}: "nessa Studio" at x ${studio[0]} is under the toggle`)
        // An item's full view puts "nessa Studio" where the sidebar's foot does.
        if (!now.sidebar) continue
        const roomy =
          (await page.locator(css.sideRailToggle).getAttribute("aria-disabled")) !==
          "true"
        if (!now.rail && !roomy) continue
        if (!now.rail) await setRail(page, true)
        await page.locator(selectorFor.railItem("notes")).click()
        if (!now.rail) await setRail(page, false)
        await rest(page, size)
        const inView = await boxOf(page, css.studio)
        const viewToggle = await boxOf(page, css.sideRailToggle)
        seen.push({ label: `${label}, Notes`, toggle: viewToggle, studio: inView })
        if (inView?.join() !== studio?.join())
          failures.push(
            `${label}: "nessa Studio" is at [${inView}] in a full view, [${studio}] in Agents`,
          )
        if (viewToggle?.join() !== toggle.join())
          failures.push(
            `${label}, Notes: the toggle is at [${viewToggle}], not [${toggle}]`,
          )
        if (!now.rail) await setRail(page, true)
        await page.locator(selectorFor.railItem("agents")).click()
        if (!now.rail) await setRail(page, false)
        await rest(page, size)
      }
    return { failures, detail: seen }
  },

  "titlebar-steady": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    const railDrawn = await page.evaluate(
      (frame) => document.querySelector(frame)?.dataset.rail === "open",
      css.workspaceWindow,
    )
    if (!railDrawn) return "skipped: no room for the side rail at this size"
    const failures = []
    const runs = []
    for (const step of ["close", "open"]) {
      await page.evaluate((sel) => {
        window.__titlebarFrames = []
        const start = performance.now()
        const tick = () => {
          const control = document.querySelector(sel)
          window.__titlebarFrames.push(control?.getBoundingClientRect().left ?? null)
          if (performance.now() - start < 700) requestAnimationFrame(tick)
        }
        requestAnimationFrame(tick)
      }, css.titlebarSidebarToggle)
      await page.locator(css.sideRailToggle).click()
      await rest(page, size)
      const frames = await page.evaluate(() => window.__titlebarFrames)
      const from = frames[0]
      const to = frames.at(-1)
      const low = Math.min(from, to) - 1
      const high = Math.max(from, to) + 1
      const strays = frames.filter((x) => x < low || x > high)
      runs.push({ step, from, to, frames: frames.length })
      if (strays.length)
        failures.push(
          `${step}: ${strays.length} frame(s) drew the controls at ${strays.map(Math.round)} — outside ${Math.round(from)}…${Math.round(to)}`,
        )
    }
    return { runs, failures }
  },

  "rail-off": async ({ page }) => {
    const seen = await page.evaluate(
      ({ rail, toggle, frame, workspace, identity }) => ({
        rail: document.querySelector(rail) !== null,
        toggle: document.querySelector(toggle) !== null,
        mode: document.querySelector(frame)?.dataset.rail,
        workspaceLeft: document.querySelector(workspace)?.getBoundingClientRect().left,
        footerPadding: getComputedStyle(document.querySelector(identity)).paddingLeft,
      }),
      {
        rail: css.sideRail,
        toggle: css.sideRailToggle,
        frame: css.workspaceWindow,
        workspace: css.workspace,
        identity: ".workspace-identity",
      },
    )
    const failures = []
    if (seen.rail) failures.push("the rail is drawn with the preview off")
    if (seen.toggle) failures.push("the rail's toggle is drawn with the preview off")
    if (seen.mode !== "off")
      failures.push(`the frame says data-rail=${seen.mode}, not off`)
    if (seen.workspaceLeft !== 0)
      failures.push(
        `the workspace begins at ${seen.workspaceLeft}, not the window's edge`,
      )
    return { ...seen, failures }
  },

  "rail-view-inert": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    const railDrawn = await page.evaluate(
      (frame) => document.querySelector(frame)?.dataset.rail === "open",
      css.workspaceWindow,
    )
    if (!railDrawn) return "skipped: no room for the side rail at this size"
    const state = () =>
      page.evaluate(
        ([workspace, overview]) => ({
          sidebar: document.querySelector(workspace)?.dataset.sidebar,
          list: document.querySelector(workspace)?.dataset.list,
          overview: document.querySelector(overview) !== null,
        }),
        [css.workspace, css.overview],
      )
    const before = await state()
    await page.locator(selectorFor.railItem("notes")).click()
    await rest(page, size)
    for (const key of [keys.toggleSidebar, keys.overview, keys.toggleSessionList])
      await page.keyboard.press(key)
    await rest(page, size)
    const under = await state()
    await page.locator(selectorFor.railItem("agents")).click()
    await rest(page, size)
    const after = await state()
    const failures = []
    for (const [name, value] of Object.entries(before)) {
      if (under[name] !== value)
        failures.push(`under Notes, ${name} became ${under[name]} (was ${value})`)
      if (after[name] !== value)
        failures.push(`back in Agents, ${name} is ${after[name]} (was ${value})`)
    }
    return { before, under, after, failures }
  },
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.list(options.only) : checks
  const sizes = options.choices("sizes").map((s) => {
    const [width, height] = s.split("x").map(Number)
    return { width, height }
  })
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const size of sizes)
        for (const name of only) {
          const base = { engine, layout, size: `${size.width}x${size.height}` }
          await attempt(rep, { ...base, name }, async () => {
            const opened = await openPage(browser, {
              url,
              layout,
              ...size,
              prefs: name === "rail-off" ? {} : { [storage.sideRail]: "on" },
            })
            try {
              await need(opened.page, css.pane, "a pane")
              if (name !== "rail-off")
                await need(opened.page, css.sideRailToggle, "the side rail's toggle")
              return await body[name]({ page: opened.page, layout, size })
            } finally {
              await opened.close()
            }
          })
        }
  })
})
