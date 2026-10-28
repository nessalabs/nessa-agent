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
 *   rail-corner      the toggle holds its bottom-left slot; the sidebar
 *                    reserves space beside it when the rail is folded.
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
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names, selectorFor, storage } from "./lib/selectors.mjs"
import { frames, settled, openPanes } from "./lib/workspace.mjs"

const checks = [
  "window-tabs",
  "shell-chrome",
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
  pane-level      focus changes only the rim, not pane fill or elevation, in both themes
  content-visible pane bodies and fill stay visible through toggles and a new pane mounts fully opaque at unit scale
  no-overlap       every combination of the rail, the sidebar, the session list
                   (columns layout) and the overview leaves the drawn columns in
                   order, none overlapping the next, none past the window's edges
  rail-corner      the bottom-left toggle holds its slot; "nessa Studio"
                   stays clear in Agents and a full view
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
                   room, hands the keyboard to the toggle`,
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

/** Sets the rail and the sidebar; the rail first, then the sidebar. */
async function arrange(page, size, { rail, sidebar }) {
  if (!(await page.locator(css.sidebar).isVisible()))
    await page.keyboard.press(keys.toggleSidebar)
  await settled(page)
  await setRail(page, rail)
  if (!sidebar) await page.keyboard.press(keys.toggleSidebar)
  await rest(page, size)
}

const body = {
  "window-tabs": async ({ page, size }) => {
    const failures = []
    const tabs = page.getByRole("tablist", { name: "Open views" })
    const selected = () => tabs.locator('[role="tab"][aria-selected="true"]')
    const visible = () =>
      page.locator(css.pane).evaluateAll((panes) =>
        panes.map((e) => {
          const b = e.getBoundingClientRect()
          return {
            key: e.dataset.paneKey,
            x: b.x,
            y: b.y,
            width: b.width,
            height: b.height,
          }
        }),
      )
    // Fold side columns so the same explicit split can fit at each test size.
    if (await page.locator(css.sidebar).isVisible())
      await page.keyboard.press(keys.toggleSidebar)
    if (await page.locator(css.sessionList).isVisible())
      await page.keyboard.press(keys.toggleSessionList)
    await settled(page)
    const firstTitle = await selected().innerText()
    await page.getByRole("button", { name: /^New Tab/ }).click()
    const draft = await selected().locator("..").getAttribute("data-session-tab")
    await page.locator(css.composer).fill("A thought kept between tabs")
    await tabs.getByRole("tab", { name: firstTitle, exact: true }).click()
    await tabs.locator(`[data-session-tab="${draft}"] [role="tab"]`).click()
    if ((await page.locator(css.composer).inputValue()) !== "A thought kept between tabs")
      failures.push("tab switching lost an unsent draft")
    await page.getByRole("button", { name: /^Split Right/ }).click()
    await settled(page)
    const split = await visible()
    if (split.length !== 2)
      failures.push(`Split Right made ${split.length} panes, expected two`)
    const secondDraft = await selected().locator("..").getAttribute("data-session-tab")
    await tabs.getByRole("tab", { name: firstTitle, exact: true }).click()
    await settled(page)
    const switched = await visible()
    if (JSON.stringify(split) !== JSON.stringify(switched))
      failures.push("switching a document changed the split arrangement")
    await tabs.locator(`[data-session-tab="${draft}"] [role="tab"]`).click()
    await settled(page)
    if ((await page.locator(css.pane).count()) !== 2)
      failures.push("selecting a visible document duplicated or closed a pane")
    await tabs
      .locator(`[data-session-tab="${secondDraft}"]`)
      .getByRole("button", { name: /^Close Tab/ })
      .click()
    await settled(page)
    if (await tabs.locator(`[data-session-tab="${secondDraft}"]`).count())
      failures.push("closed document still has a tab")
    // Roving keyboard selection must reach each document and bring it into view.
    await selected().focus()
    await page.keyboard.press("Home")
    const firstDocument = await tabs.getByRole("tab").first().innerText()
    if ((await selected().innerText()) !== firstDocument)
      failures.push("Home did not select the first document")
    if (await tabs.getByRole("tab", { name: "Agents", exact: true }).count())
      failures.push("Agents still occupies a permanent document tab")
    await page.keyboard.press("End")
    if (
      (await selected().innerText()) !== (await tabs.getByRole("tab").last().innerText())
    )
      failures.push("End did not select the last document")
    await page.keyboard.press(keys.overview)
    await need(page, css.overview, "Agents overview opened above the document tabs")
    await page
      .locator(".workspace-titlebar-navigation button:not(:disabled)")
      .last()
      .focus()
    await page.keyboard.press("Tab")
    const keyboardTab = await page.evaluate(
      () =>
        document.activeElement?.getAttribute("role") === "tab" &&
        !!document.activeElement?.closest('[role="tablist"]'),
    )
    if (!keyboardTab) failures.push("document tabs cannot be reached from Agents by Tab")
    else {
      await page.keyboard.press("ArrowLeft")
      if ((await selected().innerText()) !== firstTitle)
        failures.push("a keyboard tab choice did not leave Agents for its document")
    }
    const geometry = await page.locator(css.titlebar).evaluate((header) => {
      const list = header.querySelector('[role="tablist"]')
      const tab = list.querySelector('[aria-selected="true"]')
      const rect = (e) => {
        if (!e) return null
        const b = e.getBoundingClientRect()
        return { left: b.left, right: b.right }
      }
      return {
        list: rect(list),
        selected: rect(tab),
        tabs: [...list.querySelectorAll(".workspace-window-tab")].map((e) => ({
          width: e.getBoundingClientRect().width,
          overflow: getComputedStyle(e.querySelector('[role="tab"] > span:last-child'))
            .textOverflow,
        })),
        controls: [...header.querySelectorAll(".workspace-window-tabs > button")].map(
          (e) => {
            const b = e.getBoundingClientRect()
            return {
              ...rect(e),
              hit:
                document
                  .elementFromPoint(b.x + b.width / 2, b.y + b.height / 2)
                  ?.closest("button") === e,
            }
          },
        ),
      }
    })
    if (new Set(geometry.tabs.map((tab) => tab.width)).size !== 1)
      failures.push("document tabs have different widths")
    if (geometry.tabs.some((tab) => tab.overflow !== "ellipsis"))
      failures.push("long document titles do not truncate with an ellipsis")
    if (
      !geometry.selected ||
      geometry.selected.left < geometry.list.left - 1 ||
      geometry.selected.right > geometry.list.right + 1
    )
      failures.push("selected tab is clipped by horizontal overflow")
    if (
      geometry.controls.some(
        (b) => !b.hit || b.right > size.width || b.left < geometry.list.right - 1,
      )
    )
      failures.push("tab overflow covers New Tab or Split Right")
    if (keyboardTab) {
      await page
        .locator(".workspace-titlebar-navigation button:not(:disabled)")
        .last()
        .focus()
      await page.keyboard.press("Tab")
      await page.keyboard.press("ArrowRight")
      await settled(page)
      if ((await selected().locator("..").getAttribute("data-session-tab")) !== draft)
        failures.push("keyboard could not reselect the draft document")
      await page
        .locator(".workspace-titlebar-navigation button:not(:disabled)")
        .last()
        .focus()
      await page.keyboard.press("Tab")
      await page.keyboard.press("Tab")
      const closeFocus = await page.evaluate(() => ({
        role: document.activeElement?.getAttribute("role"),
        label: document.activeElement?.getAttribute("aria-label"),
      }))
      const closeByKeyboard = closeFocus.label?.startsWith("Close Tab:")
      if (!closeByKeyboard)
        failures.push(
          `Close Tab cannot be reached by keyboard: ${JSON.stringify(closeFocus)}`,
        )
      else {
        await page.keyboard.press("Enter")
        await settled(page)
        if (await tabs.locator(`[data-session-tab="${draft}"]`).count())
          failures.push("keyboard Close Tab left the draft document open")
        const focusKept = await page.evaluate(
          () => document.activeElement?.getAttribute("role") === "tab",
        )
        if (!focusKept) failures.push("keyboard Close Tab lost toolbar focus")
      }
    }
    return { split, switched, keyboardTab, geometry, failures }
  },

  "shell-chrome": async ({ page, size }) => {
    const failures = []
    const states = []
    for (const rail of [false, true])
      for (const sidebar of [true, false]) {
        await arrange(page, size, { rail, sidebar })
        const state = await page.evaluate((sel) => {
          const rect = (e) => {
            const b = e?.getBoundingClientRect()
            return b
              ? {
                  x: b.x,
                  y: b.y,
                  right: b.right,
                  bottom: b.bottom,
                  width: b.width,
                  height: b.height,
                }
              : null
          }
          const titlebar = document.querySelector(sel.titlebar)
          const toggle = document.querySelector(sel.sideRailToggle)
          const sidebar = document.querySelector(sel.sidebar)
          const identity = sidebar?.querySelector(".workspace-identity")
          return {
            titlebar: rect(titlebar),
            pane: rect(document.querySelector(sel.pane)),
            toggle: rect(toggle),
            newSessionControl:
              titlebar.querySelector('[aria-label^="New Session"]') !== null,
            sidebar:
              sidebar?.getAttribute("aria-hidden") === "true" ? null : rect(sidebar),
            footerInset: identity
              ? parseFloat(getComputedStyle(identity).paddingLeft)
              : null,

            controlInset: parseFloat(
              getComputedStyle(
                document.querySelector(sel.workspaceWindow),
              ).getPropertyValue("--desktop-control-inset"),
            ),
            rail: rect(document.querySelector(sel.sideRail)),
            drawnRail:
              document.querySelector(sel.workspaceWindow).dataset.rail === "open",
          }
        }, css)
        states.push({ requestedRail: rail, requestedSidebar: sidebar, ...state })
        const label = `rail ${rail}, sidebar ${sidebar}`
        if (state.newSessionControl)
          failures.push(`${label}: New Session remains in the window toolbar`)
        if (state.titlebar?.x !== 0 || state.titlebar?.right !== size.width)
          failures.push(`${label}: titlebar does not span the window`)
        if (state.sidebar && state.sidebar.y !== 0)
          failures.push(`${label}: sidebar has an extra top row (${state.sidebar.y}px)`)
        if (state.pane && state.pane.y < state.titlebar.bottom)
          failures.push(`${label}: conversation card overlaps the window toolbar`)
        if (
          state.toggle &&
          (state.toggle.x !== 10 || state.toggle.bottom !== size.height - 10)
        )
          failures.push(`${label}: rail toggle left its bottom-left control slot`)
        const expectedInset =
          state.drawnRail || !state.sidebar
            ? state.controlInset
            : state.toggle.right - state.sidebar.x + state.controlInset
        if (state.sidebar && state.footerInset !== expectedInset)
          failures.push(
            `${label}: footer inset ${state.footerInset}, expected ${expectedInset} for its toggle slot`,
          )
        if (state.rail?.width && state.rail.width !== 48)
          failures.push(`${label}: rail width ${state.rail.width}, expected 48`)
      }
    if (failures.length) return { states, failures }
    await arrange(page, size, { rail: false, sidebar: true })
    // The window can fold the sidebar for room; only a drawn footer can be clicked.
    const footerDrawn = await page.locator(css.studio).isVisible()
    if (footerDrawn) {
      await page.locator(css.studio).click()
      await need(page, css.settings, "Settings opened from the footer")
      await page.getByRole("button", { name: names.leaveSettings }).first().click()
      await page.locator(css.settings).waitFor({ state: "hidden" })
    }
    await page.keyboard.press(keys.newSession)
    await need(page, css.paneHome, "New Session opened with its keyboard shortcut")
    return { states, footerDrawn, failures }
  },

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
        if (!at) failures.push(`${label}: the toggle is not drawn`)
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
          {
            titlebar: css.titlebar,
            controls: `${css.titlebar} .workspace-titlebar-navigation .desktop-icon-button, ${css.titlebar} .workspace-window-tabs > .desktop-icon-button`,
          },
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
    const cardTop = await page
      .locator(".agents-overview-surface")
      .evaluate((e) => e.getBoundingClientRect().top)
    const toolbarBottom = await page
      .locator(css.titlebar)
      .evaluate((e) => e.getBoundingClientRect().bottom)
    const failures = reachable.length
      ? [`under the overview, not inert: ${[...new Set(reachable)].join(", ")}`]
      : []
    if (cardTop < toolbarBottom) failures.push("Agents card overlaps the window toolbar")
    return { reachable, cardTop, toolbarBottom, failures }
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
    const pluginChrome = await page.evaluate(() => {
      const view = document.querySelector(".rail-view")
      const bounds = view.getBoundingClientRect()
      const hit = document.elementFromPoint(bounds.x + bounds.width / 2, 12)
      const dragSpace = document.querySelector(".workspace-titlebar-space")
      const dragBounds = dragSpace.getBoundingClientRect()
      const dragHit = document.elementFromPoint(
        dragBounds.x + dragBounds.width / 2,
        dragBounds.y + dragBounds.height / 2,
      )
      return {
        top: bounds.top,
        coversHeader:
          view === hit || view.contains(hit) || !!hit?.closest(".workspace-titlebar"),
        draggableHeader: dragHit?.hasAttribute("data-tauri-drag-region") ?? false,
        navigation: [...document.querySelectorAll(".workspace-titlebar button")].map(
          (e) => e.getAttribute("aria-controls"),
        ),
      }
    })
    const pluginToggles = []
    for (const id of ["notes", "calendar", "browser", "files"]) {
      await page.locator(selectorFor.railItem(id)).click()
      const topToggle = page.locator(".workspace-titlebar button")
      await topToggle.click()
      const closed = await expanded(page, css.sideRailToggle)
      await topToggle.click()
      const reopened = await expanded(page, css.sideRailToggle)
      pluginToggles.push({ id, closed, reopened })
    }
    await page.locator(selectorFor.railItem("agents")).click()
    await rest(page, size)
    const after = await state()
    const failures = []
    for (const toggle of pluginToggles)
      if (toggle.closed || !toggle.reopened)
        failures.push(
          `${toggle.id}: top sidebar toggle does not fold and reopen the rail`,
        )
    if (
      pluginChrome.top !== 0 ||
      !pluginChrome.coversHeader ||
      pluginChrome.navigation.join() !== "side-rail"
    )
      failures.push("plugin view exposes the workspace cards or controls above it")
    if (!pluginChrome.draggableHeader)
      failures.push("plugin titlebar has no window drag surface")
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
