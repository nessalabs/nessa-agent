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
 *                    between where they were and where they land.
 *   titlebar-row     the titlebar's controls are centred 24px below the window's
 *                    top, the line the host centres the traffic lights on
 *                    (`src-tauri/src/platform/macos/traffic_lights.rs`), in
 *                    every frame of a sidebar toggle, with the rail open and closed.
 *   rail-off         the rail is a preview, off until Settings › Advanced ›
 *                    Experimental turns it on: off, there is no rail and no
 *                    toggle, nothing sits where the rail would, and "nessa
 *                    Studio" is in the sidebar's foot as without the rail.
 *
 * Every check but rail-off runs with the rail turned on.
 */
import { sessionRowForRemoval } from "./lib/session-row-target.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, selectorFor, storage } from "./lib/selectors.mjs"
import {
  flatSurfaceFailures,
  frames,
  openPanes,
  paneCountIs,
  settled,
} from "./lib/workspace.mjs"

const checks = [
  "no-overlap",
  "content-visible",
  "pane-level",
  "rail-corner",
  "rail-view-inert",
  "rail-loading",
  "titlebar-steady",
  "titlebar-row",
  "rail-off",
  "overview-covers",
  "rail-fold-focus",
  "draft-held",
  "reply-held",
  "stop-held",
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
  pane-level      the panes are one flat surface, divided by hairlines, and focus
                  changes no pane's fill or elevation, in both themes, beside and
                  stacked
  content-visible pane bodies and fill stay visible through toggles and a new pane mounts fully opaque at unit scale
  no-overlap       every combination of the rail, the sidebar, the session list
                   (columns layout) and the overview leaves the drawn columns in
                   order, none overlapping the next, none past the window's edges
  rail-corner      the toggle never moves and hides only with rail and sidebar
                   both closed; "nessa Studio" holds its place between Agents
                   and a full view and is never under the toggle
  rail-view-inert  ⌘B, ⌘0 and ⌥⌘S under a full view change nothing behind it
  rail-loading     an item's view opens empty; still loading at 3 s it shows
                   only the kit's sweeping text, with no panel or border
  titlebar-steady  through a rail toggle the titlebar's controls stay between
                   where they were and where they land, every frame
  titlebar-row     the titlebar's controls are centred 24px down, on the traffic
                   lights' line, in every frame of ⌘B closing and opening the sidebar
  rail-off         with the preview off (the default): no rail, no toggle
  overview-covers  with the Agents overview open, the session list and the
                   panes under it are inert
  rail-fold-focus  a rail button with the keyboard on it, folded away for
                   room, hands the keyboard to the toggle
  draft-held       closing a new session that is not the last pane keeps its
                   home and unsent text on the next frame, until the box goes
  reply-held       the source removing an established session that is not the
                   last pane keeps its unsent reply on the next frame
  stop-held        the source removing a session whose agent is at work, and
                   that is not the last pane, keeps Stop and that turn on the
                   next frame`,
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
    // Under the open overview the list and the chat area keep their layout,
    // covered rather than hidden (`data-overview-covered`, `overview-layer.tsx`).
    if (element.closest("[inert]")) continue
    if (
      (name === "list" || name === "chat") &&
      element.closest("[data-overview-covered]")
    )
      continue
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
          // Closed by the person, not folded by the window for room.
          sidebarChosen: window?.dataset.sidebarChosen === "open",
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
        const hidden = !now.rail && !now.sidebarChosen
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
    if (!railDrawn) throw new CannotRun("no room for the side rail at this size")
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
      if (!frames.length || frames.some((x) => typeof x !== "number"))
        throw new Error(`${step}: the titlebar's sidebar toggle was not measured`)
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

  "pane-level": async ({ page }) => {
    await openPanes(page, 2)
    const failures = []
    const runs = []
    for (const theme of ["dark", "light"]) {
      await page.evaluate(
        (theme) => document.documentElement.classList.toggle("dark", theme === "dark"),
        theme,
      )
      for (const focused of [0, 1]) {
        await page.locator(css.composer).nth(focused).focus()
        await settled(page)
        const surfaces = await page.evaluate(
          (selector) =>
            [...document.querySelectorAll(selector)].map((element) => {
              const style = getComputedStyle(element)
              return {
                fill: style.backgroundColor,
                shadow: style.boxShadow
                  .split(/,(?![^()]*\))/)
                  .filter((part) => !part.includes("inset"))
                  .map((part) => part.trim())
                  .join(","),
              }
            }),
          css.pane,
        )
        runs.push({ theme, focused, surfaces })
        if (
          surfaces.length !== 2 ||
          surfaces[0].fill !== surfaces[1].fill ||
          surfaces[0].shadow !== surfaces[1].shadow
        )
          failures.push(
            `${theme}, focus ${focused}: panes have different fill or elevation`,
          )
      }
    }
    // Beside, and one of the two split in a row: every hairline meets an edge,
    // in both themes.
    const flatIn = async (shape) => {
      for (const theme of ["dark", "light"]) {
        await page.evaluate(
          (theme) => document.documentElement.classList.toggle("dark", theme === "dark"),
          theme,
        )
        await settled(page)
        for (const broken of await flatSurfaceFailures(page))
          failures.push(`${theme}, ${shape}: ${broken}`)
      }
    }
    await flatIn("beside")
    await page.locator(css.composer).nth(1).focus()
    await page.keyboard.press(keys.splitDown)
    await paneCountIs(page, 3)
    await settled(page)
    // A narrow window stacks every pane, so there may be more than one row.
    const rows = await page.locator(`${css.chat} ${css.paneEdge}[data-axis="y"]`).count()
    if (rows < 1) failures.push(`⇧⌘\\ left no row hairline`)
    await flatIn("stacked")
    return { runs, failures }
  },

  "content-visible": async ({ page, layout }) => {
    const runs = []
    const failures = []
    const actions = [keys.toggleSidebar, keys.toggleSidebar]
    if (layout === "columns") actions.push(keys.toggleSessionList, keys.toggleSessionList)
    actions.push("mount filled pane")
    for (const key of actions) {
      const mounting = key === "mount filled pane"
      await settled(page)
      await page.evaluate(
        ({ pane, body, header, flight, mounting }) => {
          // Each pane with the opacity of every box from its body up to it, so a
          // partial dim fails as surely as a blackout.
          const observed = [...document.querySelectorAll(pane)].map((element) => {
            const chain = []
            for (
              let at = element.querySelector(body);
              at && at !== element.parentElement;
              at = at.parentElement
            )
              chain.push({ at, opacity: Number(getComputedStyle(at).opacity) })
            return { element, fill: getComputedStyle(element).backgroundColor, chain }
          })
          const initialPanes = new Set(observed.map(({ element }) => element))
          const chrome = document.querySelector("[data-sidebar]")
          window.__contentFrames = {
            running: true,
            frames: 0,
            failures: [],
            before: `${chrome?.dataset.sidebar}/${document.querySelector("[data-list]")?.dataset.list}`,
            geometry: new Set(),
            flights: 0,
            mounted: new Set(),
            mountedFrames: 0,
            minimumMountedOpacity: 1,
          }
          const sample = () => {
            const result = window.__contentFrames
            if (!result.running) return
            result.frames++
            if (document.querySelector(flight)) result.flights++
            const first = observed[0]?.element.getBoundingClientRect()
            if (first)
              result.geometry.add(`${first.left.toFixed(1)},${first.width.toFixed(1)}`)
            for (const { element, fill, chain } of observed) {
              if (!element.isConnected) {
                result.failures.push("pane was replaced during a chrome toggle")
                continue
              }
              const contents = element.querySelector(body)
              if (!contents) {
                result.failures.push("pane body disappeared")
                continue
              }
              for (
                let at = contents;
                at && at !== element.parentElement;
                at = at.parentElement
              ) {
                const style = getComputedStyle(at)
                if (
                  style.contentVisibility === "hidden" ||
                  style.visibility === "hidden" ||
                  style.display === "none" ||
                  Number(style.opacity) === 0
                )
                  result.failures.push(
                    `${at.className}: content hidden during transition`,
                  )
              }
              for (const { at, opacity } of chain)
                if (at.isConnected && Number(getComputedStyle(at).opacity) < opacity)
                  result.failures.push(
                    `${at.className}: content dimmed during transition`,
                  )
              if (getComputedStyle(element).backgroundColor !== fill)
                result.failures.push("pane fill changed during transition")
            }
            if (mounting)
              for (const element of document.querySelectorAll(pane)) {
                if (initialPanes.has(element)) continue
                result.mounted.add(element.dataset.paneKey)
                result.mountedFrames++
                for (const target of [
                  element.querySelector(body),
                  element.querySelector(header),
                ]) {
                  if (!target) {
                    result.failures.push("mounted pane body or header is missing")
                    continue
                  }
                  let opacity = 1
                  for (let at = target; at; at = at.parentElement) {
                    const style = getComputedStyle(at)
                    opacity *= Number(style.opacity)
                    if (
                      style.display === "none" ||
                      style.visibility !== "visible" ||
                      style.contentVisibility === "hidden"
                    )
                      result.failures.push("mounted pane body or header is hidden")
                  }
                  result.minimumMountedOpacity = Math.min(
                    result.minimumMountedOpacity,
                    opacity,
                  )
                  if (opacity !== 1)
                    result.failures.push("mounted pane body or header is dimmed")
                }
                for (const target of [element, element.querySelector(header)]) {
                  if (!target) continue
                  const transform = getComputedStyle(target).transform
                  if (
                    transform !== "none" &&
                    !new DOMMatrixReadOnly(transform).isIdentity
                  )
                    result.failures.push("mounted pane root or header is transformed")
                }
              }
            requestAnimationFrame(sample)
          }
          sample()
        },
        {
          pane: css.pane,
          body: css.paneBody,
          header: css.paneHeader,
          flight: css.workspaceFlight,
          mounting,
        },
      )
      try {
        if (mounting) await openPanes(page, 2)
        else await page.keyboard.press(key)
        await settled(page)
        await frames(page)
        const result = await page.evaluate(() => {
          const result = window.__contentFrames
          result.running = false
          const chrome = document.querySelector("[data-sidebar]")
          return {
            frames: result.frames,
            failures: result.failures,
            before: result.before,
            after: `${chrome?.dataset.sidebar}/${document.querySelector("[data-list]")?.dataset.list}`,
            positions: result.geometry.size,
            flights: result.flights,
            mounted: [...result.mounted],
            mountedFrames: result.mountedFrames,
            minimumMountedOpacity: result.minimumMountedOpacity,
          }
        })
        runs.push({
          key,
          frames: result.frames,
          positions: result.positions,
          flights: result.flights,
          mounted: result.mounted,
          mountedFrames: result.mountedFrames,
          minimumMountedOpacity: result.minimumMountedOpacity,
        })
        if (mounting && (result.mounted.length !== 1 || result.mountedFrames < 2))
          failures.push("split did not mount and sample one new filled pane")
        if (result.frames < 2) failures.push(`${key}: no transition frames sampled`)
        // The toggle changes the bounds directly; no workspace FLIP owns an
        // intermediate transform. Keep observing every independent CSS frame.
        if (!mounting && result.before === result.after)
          failures.push(`${key}: the toggle changed nothing`)
        else if (!mounting && result.positions < 2)
          failures.push(`${key}: the pane bounds changed nothing`)
        if (result.flights > 0) failures.push(`${key}: workspace bounds started a FLIP`)
        failures.push(
          ...[...new Set(result.failures)].map((message) => `${key}: ${message}`),
        )
      } finally {
        await page.evaluate(() => {
          window.__contentFrames.running = false
        })
      }
    }
    try {
      // The pane already open still shows a finished scene. The one just
      // mounted is still taking its frames. Returning on the first finished
      // scene would read the new one before it is ready.
      await page.waitForFunction(
        () => {
          const shown = [...document.querySelectorAll(".desktop-night-scene")].filter(
            (scene) => {
              const box = scene.getBoundingClientRect()
              return box.width > 0 && box.height > 0
            },
          )
          return (
            shown.length > 0 &&
            shown.every((scene) => scene.hasAttribute("data-scene-complete"))
          )
        },
        null,
        { timeout: 1000, polling: "raf" },
      )
    } catch {
      failures.push("night scene did not finish laying out after the split settled")
    }
    const scenes = await page.evaluate(() =>
      [...document.querySelectorAll(".desktop-night-scene")].map((scene) => {
        const box = scene.getBoundingClientRect()
        const layers = scene.querySelector(".desktop-night-scene-layers")
        return {
          shown: box.width > 0 && box.height > 0,
          ready: scene.hasAttribute("data-scene-ready"),
          complete: scene.hasAttribute("data-scene-complete"),
          skipped: getComputedStyle(scene).contentVisibility === "hidden",
          width: layers ? layers.getBoundingClientRect().width : 0,
        }
      }),
    )
    const shown = scenes.filter((scene) => scene.shown)
    if (shown.length === 0) failures.push("no night scene was shown after the split")
    for (const scene of shown) {
      if (!scene.ready || !scene.complete)
        failures.push("night scene stayed unready after the split settled")
      if (scene.skipped)
        failures.push("night scene text stayed skipped after the split settled")
      if (scene.width < 1) failures.push("night scene text has no width")
    }
    return { runs, failures }
  },

  "titlebar-row": async ({ page, size }) => {
    const failures = []
    const runs = []
    for (const rail of [true, false]) {
      await arrange(page, size, { rail, sidebar: true })
      for (const step of ["close", "open"]) {
        await page.evaluate(
          (sel) => {
            window.__titlebarRows = []
            const start = performance.now()
            const tick = () => {
              const titlebar = document.querySelector(sel.titlebar)
              const half = titlebar ? titlebar.getBoundingClientRect().height / 2 : null
              const centres = [...document.querySelectorAll(sel.controls)]
                .filter((button) => getComputedStyle(button).visibility !== "hidden")
                .map((button) => {
                  const box = button.getBoundingClientRect()
                  return {
                    name: button.getAttribute("aria-label"),
                    y: box.top + box.height / 2,
                  }
                })
              window.__titlebarRows.push({ half, centres })
              if (performance.now() - start < 700) requestAnimationFrame(tick)
            }
            requestAnimationFrame(tick)
          },
          { titlebar: css.titlebar, controls: `${css.titlebar} .desktop-icon-button` },
        )
        await page.keyboard.press(keys.toggleSidebar)
        await rest(page, size)
        const frames = await page.evaluate(() => window.__titlebarRows)
        if (!frames.length || frames.some((frame) => frame.centres.length === 0))
          throw new Error(
            `rail ${rail ? "open" : "closed"}, ${step}: no titlebar control measured`,
          )
        runs.push({ rail, step, frames: frames.length })
        // 24px is the native row centre too: the host reads the same height
        // from styles.css (`src-tauri/titlebar_geometry.rs`).
        for (const frame of frames) {
          if (frame.half !== 24)
            failures.push(
              `rail ${rail ? "open" : "closed"}, ${step}: ` +
                (frame.half === null
                  ? "no titlebar"
                  : `the titlebar is ${frame.half * 2}px tall, not 48`),
            )
          for (const { name, y } of frame.centres)
            if (Math.abs(y - 24) > 0.5)
              failures.push(
                `rail ${rail ? "open" : "closed"}, ${step}: ${name} centred at ${y.toFixed(1)}px, not 24`,
              )
        }
      }
    }
    return { runs, failures: [...new Set(failures)] }
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

  "overview-covers": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    await page.locator(css.composer).first().focus()
    await page.keyboard.press(keys.overview)
    await need(page, css.overview, "the Agents overview")
    await rest(page, size)
    const reachable = await page.evaluate(
      (selectors) =>
        selectors.flatMap((selector) =>
          [...document.querySelectorAll(selector)]
            .filter((element) => !element.closest("[inert]"))
            .map(() => selector),
        ),
      [css.sessionList, css.pane],
    )
    return {
      reachable,
      failures: reachable.length
        ? [`under the overview, not inert: ${[...new Set(reachable)].join(", ")}`]
        : [],
    }
  },

  "rail-fold-focus": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    const railDrawn = await page.evaluate(
      (frame) => document.querySelector(frame)?.dataset.rail === "open",
      css.workspaceWindow,
    )
    if (!railDrawn) throw new CannotRun("no room for the side rail at this size")
    await page.locator(selectorFor.railItem("notes")).focus()
    // Narrowed until the rail is the column that gives way (`railFits`).
    const railAt = () =>
      page.evaluate(
        (frame) => document.querySelector(frame)?.dataset.rail,
        css.workspaceWindow,
      )
    let width = size.width
    while ((await railAt()) === "open" && width > 400) {
      width -= 20
      await page.setViewportSize({ width, height: size.height })
      await frames(page)
      await settled(page)
    }
    await rest(page, { ...size, width })
    const now = await page.evaluate(
      ([frame, rail, toggle]) => ({
        rail: document.querySelector(frame)?.dataset.rail,
        inRail: document.querySelector(rail)?.contains(document.activeElement) ?? false,
        onToggle: document.activeElement === document.querySelector(toggle),
      }),
      [css.workspaceWindow, css.sideRail, css.sideRailToggle],
    )
    await page.setViewportSize(size)
    if (now.rail === "open")
      throw new CannotRun(`the rail kept its room down to ${width}px`)
    const failures = []
    if (now.inRail) failures.push("folded away, the rail still holds the keyboard")
    else if (!now.onToggle)
      failures.push("folded away, the keyboard did not go to the toggle")
    return { width, now, failures }
  },

  "rail-view-inert": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    const railDrawn = await page.evaluate(
      (frame) => document.querySelector(frame)?.dataset.rail === "open",
      css.workspaceWindow,
    )
    if (!railDrawn) throw new CannotRun("no room for the side rail at this size")
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

  "rail-loading": async ({ page, size }) => {
    await arrange(page, size, { rail: true, sidebar: true })
    const railDrawn = await page.evaluate(
      (frame) => document.querySelector(frame)?.dataset.rail === "open",
      css.workspaceWindow,
    )
    if (!railDrawn) throw new CannotRun("no room for the side rail at this size")
    const loader = () =>
      page.evaluate(() => {
        const view = document.querySelector(".rail-view")
        const text = view?.querySelector(".rail-view-loading")
        if (!view || !text) return { view: view !== null, text: null }
        // Every box from the text up to the view: none may draw a frame.
        const framed = []
        for (let el = text; el && el !== view; el = el.parentElement) {
          const style = getComputedStyle(el)
          if (
            parseFloat(style.borderTopWidth) > 0 ||
            style.backgroundColor !== "rgba(0, 0, 0, 0)"
          )
            framed.push(el.className || el.tagName)
        }
        const sweep = text.firstElementChild
        return {
          view: true,
          text: text.textContent,
          slot: text.getAttribute("data-slot"),
          panel: view.querySelector('[data-slot="generating-surface"]') !== null,
          framed,
          sweeping: sweep ? sweep.getAnimations().length > 0 : false,
          reduced: matchMedia("(prefers-reduced-motion: reduce)").matches,
        }
      })
    await page.locator(selectorFor.railItem("files")).click()
    const opened = await loader()
    await page.waitForTimeout(3300)
    const late = await loader()
    // Back to Agents, so the sweep stops before the next check.
    await page.locator(selectorFor.railItem("agents")).click()
    const failures = []
    if (!opened.view) failures.push("choosing Files drew no view")
    if (opened.text !== null)
      failures.push(`the view opened with a loader already: "${opened.text}"`)
    if (late.text === null) failures.push("no loader 3.3 s after choosing Files")
    else {
      if (late.slot !== "loading-text")
        failures.push(`the loader is ${late.slot}, not the kit's LoadingText`)
      if (!late.text.includes("Loading Files"))
        failures.push(`the loader reads "${late.text}"`)
      if (late.panel) failures.push("a generating-surface panel is drawn")
      if (late.framed.length)
        failures.push(`boxes around the text: ${late.framed.join(", ")}`)
      if (!late.reduced && !late.sweeping) failures.push("the text is not sweeping")
    }
    return { opened, late, failures }
  },

  /**
   * A non-last draft is dropped in the close, and the box stays three
   * frames. The next frame still shows that home and the unsent text.
   */
  "draft-held": async ({ page }) => {
    const text = "unsent line"
    const before = await page.locator(css.pane).count()
    await page.keyboard.press(keys.newSessionBeside)
    await need(page, css.paneHome, "a new session's home")
    const field = page.locator(`${css.paneHome} textarea`)
    await field.fill(text)
    await frames(page, 2)
    const typed = await field.inputValue()
    if (typed !== text) throw new CannotRun(`the home field reads "${typed}"`)
    await page.evaluate(
      ({ pane, home }) => {
        window.__heldDraft = null
        window.addEventListener(
          "keydown",
          (event) => {
            if (event.code !== "KeyW" || !event.metaKey) return
            requestAnimationFrame(() => {
              window.__heldDraft = [...document.querySelectorAll(pane)].map((item) => ({
                label: item.getAttribute("aria-label"),
                home: item.querySelector(home) !== null,
                text: item.querySelector("textarea")?.value ?? "",
              }))
            })
          },
          true,
        )
      },
      { pane: css.pane, home: css.paneHome },
    )
    await page.keyboard.press(keys.closePane)
    await page.waitForFunction(() => window.__heldDraft !== null)
    const sample = await page.evaluate(() => window.__heldDraft)
    await settled(page)
    await frames(page, 4)
    const after = await page.locator(css.pane).count()
    const failures = []
    const kept = sample?.find((item) => item.text === text)
    if (!sample) failures.push("the close did not sample the next frame")
    else if (!kept)
      failures.push(`the next frame has no unsent text: ${JSON.stringify(sample)}`)
    else {
      if (!kept.home) failures.push("the held box has the text but not the home")
      if (kept.label !== "New session")
        failures.push(`the held box is labelled ${kept.label}`)
      if (sample.length < before + 1)
        failures.push(
          `the next frame has ${sample.length} panes; the close started from ${before + 1}`,
        )
    }
    if (after !== before)
      failures.push(`after the box goes, ${after} panes remain; started from ${before}`)
    return { sample, before, after, failures }
  },

  /**
   * The source removes an established session while another pane stays.
   * The box keeps the unsent reply on the next frame.
   *
   * The reply goes in the pane that was already open. The session the
   * switcher opens beside it can belong to another channel, and the list
   * shows only the channel in view.
   */
  "reply-held": async ({ page }) => {
    const text = "unsent reply"
    const before = await page.locator(css.pane).count()
    const open = page.locator(css.focusedPane).first()
    const label = await open.getAttribute("aria-label")
    const key = await open.getAttribute("data-pane-key")
    if (!label || !key) throw new CannotRun("the open pane has no label")
    await openPanes(page, before + 1)
    const opened = await page.locator(css.pane).count()
    const field = page.locator(`[data-pane-key="${key}"] textarea`).first()
    await field.click()
    await field.fill(text)
    await frames(page, 2)
    const typed = await field.inputValue()
    if (typed !== text) throw new CannotRun(`the composer reads "${typed}"`)
    await settled(page)
    await frames(page, 2)
    await page.evaluate((pane) => {
      window.__heldReply = null
      window.addEventListener(
        "click",
        (event) => {
          const target = event.target instanceof Element ? event.target : null
          const removal = target?.closest(
            "[data-session-action='archive'], [data-delete-action='confirm']",
          )
          if (!removal) return
          requestAnimationFrame(() => {
            window.__heldReply = [...document.querySelectorAll(pane)].map((item) => ({
              label: item.getAttribute("aria-label"),
              text: item.querySelector("textarea")?.value ?? "",
            }))
          })
        },
        true,
      )
    }, css.pane)
    const row = await sessionRowForRemoval(page, label)
    await row.hover()
    const archive = row.locator('[data-session-action="archive"]')
    if ((await archive.count()) > 0) await archive.click()
    else {
      await row.click({ button: "right" })
      await page.getByRole("menuitem", { name: "Delete…" }).click()
      await page.locator('[data-delete-action="confirm"]').click()
    }
    await page.waitForFunction(() => window.__heldReply !== null)
    const sample = await page.evaluate(() => window.__heldReply)
    await settled(page)
    await frames(page, 4)
    const after = await page.locator(css.pane).count()
    const failures = []
    const kept = sample?.find((item) => item.text === text)
    if (!sample) failures.push("the removal did not sample the next frame")
    else if (!kept)
      failures.push(`the next frame has no unsent reply: ${JSON.stringify(sample)}`)
    else if (kept.label !== label)
      failures.push(`the held box is labelled ${kept.label}, was ${label}`)
    if (after !== opened - 1)
      failures.push(
        `after the box goes, ${after} panes remain; the removal started from ${opened}`,
      )
    return { sample, label, before, opened, after, failures }
  },

  /**
   * The source removes a session whose agent is at work, while another
   * pane stays. The box keeps Stop and that turn on the next frame.
   * Send in its place, while the turn is still showing, is the bug.
   */
  "stop-held": async ({ page }) => {
    const title = "Split panes for the chat workspace"
    const activity = "Editing workspace-panes.tsx"
    const before = await page.locator(css.pane).count()
    let row = await sessionRowForRemoval(page, title)
    await row.click()
    await page.waitForFunction(
      ({ pane, turn }) =>
        [...document.querySelectorAll(pane)].some((item) => {
          const stop =
            item.querySelector("button.desktop-send")?.getAttribute("aria-label") ===
            "Stop"
          const live = item.querySelector(".workspace-live")?.textContent ?? ""
          return stop && live.includes(turn)
        }),
      { pane: css.pane, turn: activity },
    )
    await openPanes(page, before + 1)
    const opened = await page.locator(css.pane).count()
    const showing = await page.evaluate(
      ({ pane, turn }) =>
        [...document.querySelectorAll(pane)].some((item) => {
          const stop =
            item.querySelector("button.desktop-send")?.getAttribute("aria-label") ===
            "Stop"
          const live = item.querySelector(".workspace-live")?.textContent ?? ""
          return stop && live.includes(turn)
        }),
      { pane: css.pane, turn: activity },
    )
    if (!showing) throw new CannotRun("the open turn left the pane before the removal")
    await page.evaluate((pane) => {
      window.__heldTurn = null
      window.addEventListener(
        "click",
        (event) => {
          const target = event.target instanceof Element ? event.target : null
          const removal = target?.closest(
            "[data-session-action='archive'], [data-delete-action='confirm']",
          )
          if (!removal) return
          requestAnimationFrame(() => {
            window.__heldTurn = [...document.querySelectorAll(pane)].map((item) => ({
              label: item.getAttribute("aria-label"),
              stop:
                item.querySelector("button.desktop-send")?.getAttribute("aria-label") ??
                "",
              live: item.querySelector(".workspace-live")?.textContent ?? "",
            }))
          })
        },
        true,
      )
    }, css.pane)
    row = await sessionRowForRemoval(page, title)
    await row.hover()
    const archive = row.locator('[data-session-action="archive"]')
    if ((await archive.count()) > 0) await archive.click()
    else {
      await row.click({ button: "right" })
      await page.getByRole("menuitem", { name: "Delete…" }).click()
      await page.locator('[data-delete-action="confirm"]').click()
    }
    await page.waitForFunction(() => window.__heldTurn !== null)
    const sample = await page.evaluate(() => window.__heldTurn)
    await settled(page)
    await frames(page, 4)
    const after = await page.locator(css.pane).count()
    const failures = []
    const kept = sample?.find((item) => item.live.includes(activity))
    if (!sample) failures.push("the removal did not sample the next frame")
    else if (!kept)
      failures.push(`the next frame has no running turn: ${JSON.stringify(sample)}`)
    else {
      if (kept.stop !== "Stop")
        failures.push(`the held box offers ${kept.stop} while the turn is still showing`)
      if (kept.label !== title)
        failures.push(`the held box is labelled ${kept.label}, was ${title}`)
    }
    if (after !== opened - 1)
      failures.push(
        `after the box goes, ${after} panes remain; the removal started from ${opened}`,
      )
    return { sample, title, before, opened, after, failures }
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
