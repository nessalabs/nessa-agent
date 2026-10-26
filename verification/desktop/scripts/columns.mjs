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
 *   chat-layout      the chats have no tabs: with the side columns folded and
 *                    two panes side by side, with the rail open and closed, no
 *                    tab strip is drawn; each pane has one header, in the
 *                    titlebar's row, its controls clear of the window's own
 *                    (the cluster on the left, the panel's toggle on the
 *                    right); each pane's picture runs up to the window's top
 *                    edge; and the left pane's composer starts past the
 *                    corner mark when the rail is closed.
 *   rail-corner      the toggle holds one bottom-left place in the rail's
 *                    column, rail open or closed; with the rail closed it is
 *                    the footer's mark, right before "nessa Studio".
 *   rail-view-inert  under an item's full view the workspace takes no keys:
 *                    ⌘B, ⌘0 and ⌥⌘S change nothing behind it, and coming back
 *                    to Agents finds it as it was left.
 *   quiet-composer   three stacked panes: in a short window a pane you are not
 *                    in has a one-line composer, Stop alone while its turn
 *                    runs; going into one grows its composer out of the
 *                    corner over several frames while the others stay quiet;
 *                    in a tall window every composer is whole.
 *   overview-narrow  the Agents overview at the size checked: its cover meets
 *                    the sidebar with no strip of what lies beneath; at rest
 *                    each group's card starts below the list's top fade; a
 *                    row opened beneath itself keeps everything inside its
 *                    card.
 *   sidebar-motion   ⌘B folds and unfolds the sidebar as a drawer, on
 *                    --desktop-drawer's curve: what stands beside it (the session
 *                    list, or the panes) and the pane's composer — its left
 *                    edge and its centre — move over several frames, one way,
 *                    to where they rest; with reduced motion they land at once.
 *   split-motion     ⌘D splits the chat in front and ⌘W closes the new pane:
 *                    each pane's edges and the first composer's centre move
 *                    over several frames, one way, to where they rest; the
 *                    new pane slides in at its size; with reduced motion
 *                    each change lands at once.
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
import { readFile } from "node:fs/promises"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names, selectorFor, storage } from "./lib/selectors.mjs"
import {
  flatSurfaceFailures,
  frames,
  openPanes,
  paneCountIs,
  settled,
} from "./lib/workspace.mjs"

/**
 * What a card the list may allow (`data-whole`) cuts off: its command, the
 * box around it or its tool line, wider or taller than drawn. A card that
 * allows must show everything it allows.
 */
const cutOff = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll(".workspace-request-command[data-whole]")].flatMap(
      (box) => {
        const card = box.closest(".workspace-request")
        const parts = [
          ["command", box.querySelector(".workspace-approval-command")],
          ["box", box],
          ["tool", card?.querySelector(".workspace-request-tool")],
        ]
        return parts.flatMap(([name, element]) =>
          element &&
          (element.scrollWidth > element.clientWidth + 1 ||
            element.scrollHeight > element.clientHeight + 1)
            ? [
                `${card?.getAttribute("aria-label")}: its ${name} is cut off (${element.scrollWidth}x${element.scrollHeight} in ${element.clientWidth}x${element.clientHeight})`,
              ]
            : [],
        )
      },
    ),
  )

const checks = [
  "channels-alone",
  "list-request",
  "list-request-claude",
  "list-request-narrow",
  "quiet-composer",
  "overview-narrow",
  "sidebar-motion",
  "split-motion",
  "chat-layout",
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
  list-request    three columns: a waiting row in Needs you is a card with the
                  exact command, and Allow from it settles the review — the
                  row leaves Needs you, its card with it
  list-request-claude
                  three columns, the list at its narrowest: Claude's shell
                  review reads as the transcript card reads it, its two answers
                  each fit their words, and the card keeps under 260px
  list-request-narrow
                  three columns, the list at its narrowest: a command holds the
                  columns allowableFromList counts on; a card whose command or
                  tool is not printable ASCII offers no Allow, only Review in
                  Chat; a long tool name wraps, drawn whole
  channels-alone  three columns: the sidebar lists channels and no session
                  under them; the chosen channel's pinned sessions lead its
                  list under "Pinned", each listed once. Sessions in sidebar:
                  a channel still discloses its sessions
  pane-level      the panes are one flat surface, divided by hairlines, and focus
                  changes no pane's fill or elevation, in both themes, beside and
                  stacked
  content-visible pane bodies and fill stay visible through toggles and a new pane mounts fully opaque at unit scale
  no-overlap       every combination of the rail, the sidebar, the session list
                   (columns layout) and the overview leaves the drawn columns in
                   order, none overlapping the next, none past the window's edges
  rail-corner      the bottom-left toggle never moves as the rail or sidebar
                   toggles; "nessa Studio" stays clear in Agents and a full view
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
  "chat-layout": async ({ page, size }) => {
    const failures = []
    const states = []
    for (const rail of [true, false]) {
      await arrange(page, size, { rail, sidebar: false })
      if (await page.locator(css.sessionList).isVisible())
        await page.keyboard.press(keys.toggleSessionList)
      await openPanes(page, 2)
      await rest(page, size)
      const state = await page.evaluate(
        (sel) => {
          const box = (e) => {
            const b = e.getBoundingClientRect()
            return { left: b.left, top: b.top, right: b.right, bottom: b.bottom }
          }
          const bar = document.querySelector(sel.titlebar).getBoundingClientRect().height
          const controls = [
            ...document.querySelectorAll(
              `${sel.titlebar} .workspace-titlebar-navigation button, ${sel.titlebar} .workspace-titlebar-trailing button`,
            ),
          ]
            .filter((e) => e.getClientRects().length > 0)
            .map(box)
          const panes = [...document.querySelectorAll(sel.pane)].map((pane) => ({
            box: box(pane),
            headers: [...pane.querySelectorAll(sel.paneHeader)].map((header) => ({
              box: box(header),
              // What a header draws: its name, menu and close, never its padding.
              parts: [
                ...header.querySelectorAll("button, [data-slot], .workspace-pane-name"),
              ]
                .filter((e) => e.getClientRects().length > 0)
                .map(box),
            })),
            picture: [...pane.querySelectorAll(".desktop-header, .desktop-header-image")]
              .filter((e) => e.getClientRects().length > 0)
              .map(box)[0],
            composer: (() => {
              const c = pane.querySelector(sel.composer)?.closest(".desktop-composer")
              return c ? box(c) : null
            })(),
          }))
          const mark = document.querySelector(sel.sideRailToggle)
          return {
            rail: document.querySelector(".workspace-window")?.dataset.rail,
            bar,
            // Any tab list outside the right panel would be tabs for the chats.
            tabStrip: [...document.querySelectorAll('[role="tablist"]')].filter(
              (list) => !list.closest("#workspace-panel"),
            ).length,
            controls,
            panes,
            mark: mark && mark.getClientRects().length ? box(mark) : null,
          }
        },
        {
          titlebar: css.titlebar,
          pane: css.pane,
          paneHeader: css.paneHeader,
          composer: css.composer,
          sideRailToggle: css.sideRailToggle,
        },
      )
      states.push(state)
      const when = `rail ${state.rail}`
      if (state.tabStrip) failures.push(`${when}: a tab strip is drawn for the chats`)
      const overlaps = (a, b) =>
        a.left < b.right - 1 &&
        b.left < a.right - 1 &&
        a.top < b.bottom - 1 &&
        b.top < a.bottom - 1
      for (const [index, pane] of state.panes.entries()) {
        if (pane.headers.length !== 1)
          failures.push(`${when}: pane ${index + 1} has ${pane.headers.length} headers`)
        const header = pane.headers[0]
        if (header && (header.box.top < -1 || header.box.bottom > state.bar + 1))
          failures.push(
            `${when}: pane ${index + 1}'s header is not in the titlebar row: ${JSON.stringify(header.box)}`,
          )
        for (const part of header?.parts ?? [])
          for (const control of state.controls)
            if (overlaps(part, control))
              failures.push(
                `${when}: pane ${index + 1}'s header sits on a window control: ${JSON.stringify({ part, control })}`,
              )
        // From the window's top and its pane's own left edge: the corner
        // pane's runs behind the window's controls, cut by nothing.
        if (
          !pane.picture ||
          pane.picture.top > 0.5 ||
          pane.picture.left > pane.box.left + 0.5
        )
          failures.push(
            `${when}: pane ${index + 1}'s picture does not reach the window's top and its pane's left: ${JSON.stringify({ picture: pane.picture, pane: pane.box })}`,
          )
      }
      const left = state.panes.toSorted(
        (a, b) => (a.composer?.left ?? 0) - (b.composer?.left ?? 0),
      )[0]
      if (state.rail === "closed" && (!state.mark || !left?.composer))
        failures.push(
          `${when}: no corner mark or left composer to measure: ${JSON.stringify({ mark: state.mark, composer: left?.composer })}`,
        )
      else if (state.mark && left?.composer && overlaps(left.composer, state.mark))
        failures.push(
          `${when}: the corner mark sits on the left pane's composer: ${JSON.stringify({ mark: state.mark, composer: left.composer })}`,
        )
    }
    return { states, failures }
  },

  "shell-chrome": async ({ page, layout, size }) => {
    const failures = []
    const states = []
    let firstToggle = null
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
            // The pane runs up behind the titlebar; its header is that row.
            pane: rect(document.querySelector(`${sel.pane} ${sel.paneHeader}`)),
            toggle: rect(toggle),
            newSessionControl:
              titlebar.querySelector('[aria-label^="New Session"]') !== null,
            sidebar:
              sidebar?.getAttribute("aria-hidden") === "true" ? null : rect(sidebar),
            // Where "nessa Studio" begins, from the sidebar's content edge:
            // the sidebar reaches the window's edge with its gutter inside it.
            footerInset:
              identity && sidebar
                ? identity.getBoundingClientRect().left +
                  parseFloat(getComputedStyle(identity).paddingLeft) -
                  (sidebar.getBoundingClientRect().left +
                    parseFloat(getComputedStyle(sidebar).paddingLeft))
                : null,
            sidebarContent: sidebar
              ? sidebar.getBoundingClientRect().left +
                parseFloat(getComputedStyle(sidebar).paddingLeft)
              : null,

            controlInset: parseFloat(
              getComputedStyle(
                document.querySelector(sel.workspaceWindow),
              ).getPropertyValue("--desktop-control-inset"),
            ),
            rail: rect(document.querySelector(sel.sideRail)),
            drawnRail:
              document.querySelector(sel.workspaceWindow).dataset.rail === "open",
            drawnSidebar:
              document.querySelector(sel.workspaceWindow).dataset.sidebar === "open",
          }
        }, css)
        states.push({ requestedRail: rail, requestedSidebar: sidebar, ...state })
        const label = `rail ${rail}, sidebar ${sidebar}`
        // New Session is in the window's top row only where no session list
        // has it at its head.
        if (state.newSessionControl !== (layout !== "columns"))
          failures.push(
            `${label}: New Session ${state.newSessionControl ? "is" : "is not"} in the window toolbar in ${layout}`,
          )
        if (state.titlebar?.x !== 0 || state.titlebar?.right !== size.width)
          failures.push(`${label}: titlebar does not span the window`)
        if (state.sidebar && state.sidebar.y !== 0)
          failures.push(`${label}: sidebar has an extra top row (${state.sidebar.y}px)`)
        if (
          state.pane &&
          (state.pane.y < state.titlebar.y ||
            state.pane.bottom > state.titlebar.bottom + 1)
        )
          failures.push(`${label}: a pane's header is not in the window's top row`)
        // The toggle holds one place in every state (`side-rail.css`): the
        // rail's column, on the footer row's line, its foot 13px above the
        // row's — toggling the rail or the sidebar moves what is beside it,
        // never the toggle. The footer keeps one 10px inset, rail open or
        // closed, so its mark slot is on the rail's column (the sidebar's
        // icon column) and toggling the rail never reflows it; closed, the
        // toggle stands in that slot (`rail-corner` checks the slot).
        if (state.toggle) {
          firstToggle ??= state.toggle
          if (
            state.toggle.x !== firstToggle.x ||
            state.toggle.bottom !== size.height - 21
          )
            failures.push(
              `${label}: rail toggle at x ${state.toggle.x}, foot ${state.toggle.bottom}, not at x ${firstToggle.x} on the footer's line`,
            )
        }
        if (state.sidebar && state.drawnRail && state.footerInset !== 10)
          failures.push(
            `${label}: footer inset ${state.footerInset}, not the rail column's 10px`,
          )
        if (state.rail?.width && state.rail.width !== 64)
          failures.push(`${label}: rail width ${state.rail.width}, expected 64`)
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
    // The visible footer's mark slot, as the toggle's box would be in it.
    const markSlot = (page) =>
      page.evaluate((toggleSelector) => {
        const slot = [
          ...document.querySelectorAll(".workspace-identity .desktop-identity-mark"),
        ]
          .filter(
            (each) =>
              getComputedStyle(each).visibility !== "hidden" && !each.closest("[inert]"),
          )
          .map((each) => each.getBoundingClientRect())
          .find((r) => r.width > 0 && r.left >= 0 && r.left < window.innerWidth)
        const toggle = document.querySelector(toggleSelector)?.getBoundingClientRect()
        if (!slot || !toggle) return null
        const x = slot.left + (slot.width - toggle.width) / 2
        const y = slot.top + (slot.height - toggle.height) / 2
        return [x, y, toggle.width, toggle.height].map(Math.round)
      }, css.sideRailToggle)
    const squareShown = (page) =>
      page.evaluate(() =>
        [...document.querySelectorAll(".workspace-identity .desktop-mark")].some(
          (each) =>
            getComputedStyle(each).visibility === "visible" &&
            each.getBoundingClientRect().width > 0,
        ),
      )
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
        // One place in every state: toggling the rail or the sidebar moves
        // what is beside the toggle, never the toggle under the pointer.
        toggle ??= at
        if (at.join() !== toggle.join())
          failures.push(`${label}: the toggle is at [${at}], not [${toggle}]`)
        // In the rail's column: the rail's items stand over it.
        const item = now.rail ? await boxOf(page, selectorFor.railItem("agents")) : null
        if (item && item[0] !== at[0])
          failures.push(
            `${label}: the toggle is at x ${at[0]}, the rail's items at ${item[0]}`,
          )
        // Where the footer keeps a mark slot (the rail closed), the toggle is
        // in it, right before "nessa Studio".
        const slot = await markSlot(page)
        if (slot && at.join() !== slot.join())
          failures.push(
            `${label}: the toggle is at [${at}], not in the mark slot [${slot}]`,
          )
        if (studio && studio[0] < at[0] + at[2])
          failures.push(`${label}: "nessa Studio" at x ${studio[0]} is under the toggle`)
        if (await squareShown(page))
          failures.push(`${label}: Studio's square is drawn beside the toggle`)
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

  "quiet-composer": async ({ page }) => {
    const failures = []
    const composers = () =>
      page.evaluate(() =>
        [...document.querySelectorAll("article.workspace-pane")].map((pane) => {
          const form = pane.querySelector(".desktop-composer")
          const field = form?.querySelector("textarea")
          return {
            key: pane.dataset.paneKey,
            focused: pane.hasAttribute("data-focused"),
            running: form?.hasAttribute("data-running") ?? false,
            height: form?.getBoundingClientRect().height ?? 0,
            field: field
              ? getComputedStyle(field.closest(".desktop-composer-body")).display
              : "none",
            choices: [...(form?.querySelectorAll(".desktop-chip") ?? [])].some(
              (chip) => chip.getBoundingClientRect().width > 0,
            ),
          }
        }),
      )
    const stack = async (height) => {
      await page.setViewportSize({ width: 1200, height })
      await page.locator(css.composer).first().click()
      for (let i = 0; i < 2; i++) {
        const count = await page.locator(css.pane).count()
        await page.keyboard.press(keys.splitDown)
        await page
          .waitForFunction(
            ([sel, n]) => document.querySelectorAll(sel).length > n,
            [css.pane, count],
            { timeout: 2000 },
          )
          .catch(() => {})
      }
      await settled(page)
      await frames(page, 6)
    }
    await stack(760)
    const short = await composers()
    if (short.length < 3) throw new CannotRun(`only ${short.length} panes stacked`)
    for (const c of short.filter((c) => !c.focused)) {
      if (c.choices) failures.push(`pane ${c.key}, not focused, still shows its choices`)
      if (c.running && c.field !== "none")
        failures.push(`pane ${c.key}, running and not focused, shows more than Stop`)
      if (!c.running && c.height > 48)
        failures.push(
          `pane ${c.key}, not focused, has a ${c.height}px composer, not one line`,
        )
    }
    // Into a quiet pane: its composer grows, over several frames, from the corner.
    const quiet = short.find((c) => !c.focused)
    await page.evaluate((key) => {
      window.__grow = []
      const form = () =>
        document.querySelector(`[data-pane-key="${key}"] .desktop-composer`)
      const start = performance.now()
      const tick = () => {
        const f = form()
        window.__grow.push(f ? getComputedStyle(f).clipPath : "none")
        if (performance.now() - start < 600) requestAnimationFrame(tick)
      }
      requestAnimationFrame(tick)
    }, quiet.key)
    await page.locator(`[data-pane-key="${quiet.key}"] ${css.paneHeader}`).click()
    await page.waitForTimeout(650)
    const clips = new Set(await page.evaluate(() => window.__grow))
    if (clips.size < 3)
      failures.push(`its composer grew in ${clips.size - 1} frames, not several`)
    const after = await composers()
    const grown = after.find((c) => c.key === quiet.key)
    if (!grown?.choices)
      failures.push(`pane ${quiet.key}, focused, does not show its whole composer`)
    if (after.filter((c) => !c.focused).some((c) => c.choices))
      failures.push("a pane left behind kept its whole composer")
    // Tall: room for every composer.
    await page.setViewportSize({ width: 1200, height: 1500 })
    await settled(page)
    await frames(page, 6)
    const tall = await composers()
    if (tall.some((c) => !c.choices && !c.running))
      failures.push("in a tall window a composer is still quiet")
    return { short, after, tall, failures }
  },

  "list-request": async ({ page, layout }) => {
    if (layout !== "columns") throw new CannotRun("only three columns has a session list")
    const failures = []
    // The first card the list may allow from (`allowableFromList`).
    const allowable = page
      .locator('[data-list-group="needs-you"] .workspace-request-card')
      .filter({ has: page.locator('button[data-answer="once"]') })
      .first()
    const card = allowable.locator(".workspace-request")
    await card.waitFor({ timeout: 10_000 }).catch(() => {})
    if (!(await card.count())) throw new CannotRun("nothing in Needs you may be allowed")
    const row = allowable.locator("[data-session-row]").first()
    const id = await row.getAttribute("data-session-row")
    const command = (await card.locator(".workspace-approval-command").innerText()).trim()
    if (!command.startsWith("$")) failures.push(`the card shows no command: "${command}"`)
    failures.push(...(await cutOff(page)))
    const allow = card.locator('button[data-answer="once"]')
    const deny = card.locator('button[data-answer="deny"]')
    const [a, d] = [await allow.boundingBox(), await deny.boundingBox()]
    if (!a || !d || Math.abs(a.y - d.y) > 1)
      failures.push("Deny and Allow do not stand on one row")
    await allow.click()
    const left = await page
      .waitForFunction(
        (sessionId) =>
          !document.querySelector(
            `[data-list-group="needs-you"] [data-session-row="${CSS.escape(sessionId)}"]`,
          ),
        id,
        { timeout: 5_000 },
      )
      .then(() => true)
      .catch(() => false)
    if (!left) failures.push("after Allow, the row is still in Needs you")
    return { failures, detail: { id, command } }
  },

  "list-request-narrow": async ({ page, layout }) => {
    if (layout !== "columns") throw new CannotRun("only three columns has a session list")
    const failures = []
    await page
      .locator(".workspace-sidebar button, .workspace-sidebar [role='button']", {
        hasText: /^gateway/,
      })
      .first()
      .click()
    const edge = page.getByRole("separator", { name: "Resize Session List" })
    await edge.focus()
    for (let i = 0; i < 20; i++) await page.keyboard.press("ArrowLeft")
    const marks = page.locator('[aria-label="Request from Marks wider than the card"]')
    await marks.waitFor({ timeout: 10_000 }).catch(() => {})
    if (!(await marks.count())) throw new CannotRun("the wide marks are not in Needs you")
    await settled(page)
    // The rule's own count of columns, from the rule's source.
    const source = await readFile(
      new URL(
        "../../../src/desktop/workspace/ui/session-list/list-request.tsx",
        import.meta.url,
      ),
      "utf8",
    )
    const columns = Number(/listShownColumns = (\d+)/.exec(source)?.[1])
    const seen = await page.evaluate(() => {
      const cards = [
        ...document.querySelectorAll('[data-list-group="needs-you"] .workspace-request'),
      ].filter((card) => card.querySelector(".workspace-approval-command"))
      const pre = cards[0].querySelector(".workspace-approval-command")
      const style = getComputedStyle(pre)
      const probe = document.createElement("span")
      probe.textContent = "x".repeat(100)
      probe.style.font = style.font
      probe.style.whiteSpace = "pre"
      document.body.append(probe)
      const ch = probe.getBoundingClientRect().width / 100
      probe.remove()
      const inner =
        pre.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
      return {
        held: Math.floor(inner / ch + 0.01),
        cards: cards.map((card) => {
          const words =
            card.querySelector(".workspace-approval-command").textContent ?? ""
          const tool = card.querySelector(".workspace-request-tool")
          return {
            label: card.getAttribute("aria-label"),
            ascii: /^[\x20-\x7e\n]*$/.test(words + (tool?.textContent ?? "")),
            allows: !!card.querySelector(
              'button[data-answer="once"], button[data-answer="always"]',
            ),
            review: [...card.querySelectorAll("button")].some(
              (button) => button.textContent === "Review in Chat",
            ),
            toolLines: tool
              ? Math.round(
                  tool.getBoundingClientRect().height /
                    parseFloat(getComputedStyle(tool).lineHeight),
                )
              : 0,
          }
        }),
      }
    })
    if (!(seen.held >= columns))
      failures.push(`a command holds ${seen.held} columns, the rule counts on ${columns}`)
    for (const card of seen.cards)
      if (!card.ascii && (card.allows || !card.review))
        failures.push(
          `${card.label} is not plain ASCII, yet offers ${card.allows ? "Allow" : "no Review in Chat"}`,
        )
    const named = (label) =>
      seen.cards.find((card) => card.label === `Request from ${label}`)
    const wide = named("Marks wider than the card")
    if (!wide || wide.allows || !wide.review)
      failures.push("the wide marks are not refused from the list, read in the chat")
    const lab = named("Shell through the lab's tool")
    if (!lab?.allows) failures.push("the lab's short shell call offers no Allow")
    if (!(lab?.toolLines > 1))
      failures.push(`the lab's long tool name takes ${lab?.toolLines} line, not wrapping`)
    failures.push(...(await cutOff(page)))
    return { failures, detail: { columns, ...seen } }
  },

  "list-request-claude": async ({ page, layout }) => {
    if (layout !== "columns") throw new CannotRun("only three columns has a session list")
    const failures = []
    await page
      .locator(".workspace-sidebar button, .workspace-sidebar [role='button']", {
        hasText: /^gateway/,
      })
      .first()
      .click()
    // The list at its narrowest: its edge moved left until it holds.
    const edge = page.getByRole("separator", { name: "Resize Session List" })
    await edge.focus()
    for (let i = 0; i < 20; i++) await page.keyboard.press("ArrowLeft")
    const card = page.locator('[aria-label="Request from Reconnect storm after sleep"]')
    await card.waitFor({ timeout: 10_000 }).catch(() => {})
    if (!(await card.count()))
      throw new CannotRun("the Claude review is not in Needs you")
    await settled(page)
    const seen = await card.evaluate((element) => ({
      command: element.querySelector(".workspace-approval-command")?.textContent ?? "",
      height: element.closest(".workspace-request-card")?.getBoundingClientRect().height,
      listWidth: document.querySelector(".workspace-list")?.getBoundingClientRect().width,
      squeezed: [
        ...element.querySelectorAll(".workspace-approval-actions .workspace-button"),
      ]
        .filter((button) => {
          const words = button.querySelector("span") ?? button
          return words.scrollWidth > words.clientWidth + 1
        })
        .map((button) => button.textContent),
      answers: [...element.querySelectorAll(".workspace-approval-actions button")].map(
        (button) => button.getAttribute("aria-label") ?? button.textContent,
      ),
    }))
    if (seen.squeezed.length)
      failures.push(`answers narrower than their words: ${seen.squeezed}`)
    failures.push(...(await cutOff(page)))
    if (!(seen.height < 260)) failures.push(`the card is ${seen.height}px tall`)
    if (seen.answers.join("|") !== "No|Yes|More Answers")
      failures.push(`the answers read ${seen.answers.join("|")}`)
    // The transcript card's own reading of the same command.
    await page.locator('[data-session-row="reconnect-storm"]').first().click()
    const pane = page.locator(".workspace-approval .workspace-approval-command").first()
    await pane.waitFor({ timeout: 10_000 })
    const transcript = (await pane.textContent()) ?? ""
    if (transcript.trim() !== seen.command.trim())
      failures.push(`the card reads "${seen.command}", the transcript "${transcript}"`)
    return { failures, detail: seen }
  },

  "channels-alone": async ({ page, layout }) => {
    const failures = []
    await settled(page)
    const seen = await page.evaluate(() => {
      const rows = (scope) =>
        [...document.querySelectorAll(`${scope} [data-session-row]`)].map((row) =>
          row.getAttribute("data-session-row"),
        )
      const groups = [...document.querySelectorAll(".workspace-list [data-list-group]")]
      return {
        sidebar: rows(".workspace-sidebar"),
        list: rows(".workspace-list"),
        first: groups[0]?.getAttribute("data-list-group") ?? null,
        pinned: rows('.workspace-list [data-list-group="pinned"]'),
      }
    })
    if (layout === "columns") {
      if (seen.sidebar.length)
        failures.push(
          `the sidebar lists ${seen.sidebar.length} sessions under its channels`,
        )
      if (seen.first !== "pinned" || seen.pinned.length === 0)
        failures.push(`the list starts with ${seen.first}, not the pinned sessions`)
      if (new Set(seen.list).size !== seen.list.length)
        failures.push("a session is listed twice")
    } else if (seen.sidebar.length === 0) {
      failures.push("sessions in sidebar discloses no session under its channels")
    }
    return { failures, detail: seen }
  },

  "overview-narrow": async ({ page }) => {
    const failures = []
    await page.keyboard.press(keys.overview)
    await page.locator(".agents-overview-list").first().waitFor({ timeout: 5000 })
    await settled(page)
    const at = await page.evaluate(() => {
      const box = (e) => e?.getBoundingClientRect() ?? null
      const sidebar = box(document.querySelector(".workspace-sidebar"))
      const cover = box(document.querySelector(".workspace-overview-layer"))
      const scroll = document.querySelector(
        ".agents-overview-side > .agents-overview-scroll",
      )
      const fade = scroll
        ? parseFloat(getComputedStyle(scroll).maskImage.match(/(\d+)px/)?.[1] ?? "0")
        : 0
      const top = box(scroll)?.top ?? 0
      const cards = [...document.querySelectorAll(".agents-overview-list")]
        .map((e) => box(e).top)
        .filter((y) => y < top + 200)
      return {
        sidebar:
          sidebar && document.querySelector('.workspace[data-sidebar="open"]')
            ? sidebar.right
            : null,
        cover: cover?.left ?? null,
        fadeEnd: top + fade,
        cards,
        scrolled: scroll?.scrollTop ?? 0,
      }
    })
    if (at.sidebar !== null && Math.abs(at.sidebar - at.cover) > 1)
      failures.push(
        `the overview's cover starts at ${at.cover}, the sidebar ends at ${at.sidebar}`,
      )
    if (at.scrolled === 0)
      for (const y of at.cards)
        if (y < at.fadeEnd - 1)
          failures.push(`a card's top (${y}) is under the list's fade (to ${at.fadeEnd})`)
    // A row opened beneath itself: nothing in it past its card.
    const row = page.locator(".agents-row").first()
    await row.click()
    await page
      .locator(".agents-inline-peek")
      .first()
      .waitFor({ timeout: 3000 })
      .catch(() => {})
    await settled(page)
    const over = await page.evaluate(() => {
      const peek = document.querySelector(".agents-inline-peek")
      if (!peek) return null
      const card = peek.closest(".agents-overview-list").getBoundingClientRect()
      return [...peek.querySelectorAll("*")]
        .filter((e) => {
          if (e.closest(".agents-truncate, [data-slot='code-block']")) return false
          const b = e.getBoundingClientRect()
          return b.width > 0 && (b.right > card.right + 0.5 || b.left < card.left - 0.5)
        })
        .map((e) => `${e.tagName.toLowerCase()}.${String(e.className).split(" ")[0]}`)
    })
    if (over?.length)
      failures.push(`inside an opened row, past its card: ${over.slice(0, 4).join(", ")}`)
    return { at, failures }
  },

  "sidebar-motion": async ({ page, layout }) => {
    const failures = []
    const runs = []
    // What stands just right of the sidebar — the list's content, or the
    // first pane — and that pane's composer: its left edge and its centre.
    const watched = {
      beside: layout === "columns" ? ".workspace-list-inner" : css.pane,
      composer: css.composer,
    }
    const measure = () =>
      page.evaluate((sel) => {
        const box = (s) => document.querySelector(s)?.getBoundingClientRect()
        const beside = box(sel.beside)
        const composer = box(sel.composer)
        return {
          beside: beside?.left ?? null,
          composer: composer?.left ?? null,
          centre: composer ? composer.left + composer.width / 2 : null,
        }
      }, watched)
    const toggle = async (step) => {
      const before = await measure()
      await page.evaluate((sel) => {
        window.__sidebarFrames = []
        const start = performance.now()
        const box = (s) => document.querySelector(s)?.getBoundingClientRect()
        const tick = () => {
          const beside = box(sel.beside)
          const composer = box(sel.composer)
          window.__sidebarFrames.push({
            beside: beside?.left ?? null,
            composer: composer?.left ?? null,
            centre: composer ? composer.left + composer.width / 2 : null,
          })
          if (performance.now() - start < 900) requestAnimationFrame(tick)
        }
        requestAnimationFrame(tick)
      }, watched)
      await page.keyboard.press(keys.toggleSidebar)
      await page.waitForFunction(() => window.__sidebarFrames.length > 0)
      await settled(page)
      await frames(page, 4)
      const seen = await page.evaluate(() => window.__sidebarFrames)
      return { step, before, seen, rest: await measure() }
    }
    const judge = ({ step, before, seen, rest }, reduced) => {
      const name = `${step}${reduced ? " (reduced)" : ""}`
      for (const part of ["beside", "composer", "centre"]) {
        const values = seen
          .map((frame) => frame[part])
          .filter((x) => typeof x === "number")
        const from = before[part]
        const to = values.at(-1)
        if (typeof from !== "number" || typeof to !== "number") {
          failures.push(`${name}: ${part} was not measured`)
          continue
        }
        const between = new Set(
          values
            .filter((x) => x > Math.min(from, to) + 1 && x < Math.max(from, to) - 1)
            .map(Math.round),
        )
        runs.push({
          step,
          reduced,
          part,
          from,
          to,
          rest: rest[part],
          between: between.size,
        })
        if (Math.abs(to - rest[part]) > 1)
          failures.push(`${name}: ${part} ended at ${to}, rests at ${rest[part]}`)
        if (part === "beside" && Math.abs(from - to) < 40)
          failures.push(`${name}: nothing beside the sidebar moved`)
        if (part === "beside" && !reduced && between.size < 3)
          failures.push(
            `${name}: moved in ${between.size} in-between frames, not over several`,
          )
        if (reduced && between.size > 0)
          failures.push(
            `${name}: ${part} drew ${between.size} in-between frames, not at once`,
          )
        // One way only: never back by more than a pixel.
        const back = values.slice(1).filter((x, i) => {
          const previous = values[i]
          return to > from ? x < previous - 1 : x > previous + 1
        })
        if (back.length) failures.push(`${name}: ${part} moved back ${back.length} times`)
      }
    }
    for (const step of ["close", "open"]) judge(await toggle(step), false)
    await page.emulateMedia({ reducedMotion: "reduce" })
    await frames(page, 4)
    for (const step of ["close", "open"]) judge(await toggle(step), true)
    await page.emulateMedia({ reducedMotion: "no-preference" })
    return { runs, failures }
  },

  "split-motion": async ({ page }) => {
    const failures = []
    const runs = []
    // Each pane's edges, and the composer's centre in the pane that was there.
    const sample = () => {
      const boxes = {}
      for (const pane of document.querySelectorAll("[data-pane-key]")) {
        if (pane.closest(".split-panes-ghost")) continue
        const b = pane.getBoundingClientRect()
        boxes[`pane ${pane.dataset.paneKey} left`] = b.left
        boxes[`pane ${pane.dataset.paneKey} right`] = b.right
      }
      const field = document.querySelector('[data-pane-key="1"] textarea')
      if (field) {
        const b = field.getBoundingClientRect()
        boxes["composer centre"] = b.left + b.width / 2
      }
      return boxes
    }
    // Every step starts at rest: the last one's flights have landed.
    const atRest = () =>
      page.waitForFunction(
        () =>
          !document
            .getAnimations()
            .some(
              (a) =>
                (a.playState === "running" || a.pending) &&
                a.effect?.getTiming?.().iterations !== Infinity,
            ),
        null,
        { timeout: 3000, polling: "raf" },
      )
    const run = async (step, key, reduced) => {
      await atRest()
      const before = await page.evaluate(sample)
      await page.evaluate((source) => {
        const measure = new Function(`return (${source})()`)
        window.__splitFrames = []
        window.__splitOn = true
        const tick = () => {
          window.__splitFrames.push(measure())
          if (window.__splitOn) requestAnimationFrame(tick)
        }
        requestAnimationFrame(tick)
      }, sample.toString())
      const panes = await page.locator(css.pane).count()
      await page.keyboard.press(key)
      // The grid draws the new arrangement a few frames on; then it moves.
      await page
        .waitForFunction(
          ([sel, count]) => document.querySelectorAll(sel).length !== count,
          [css.pane, panes],
          { timeout: 2000 },
        )
        .catch(() => {})
      await settled(page)
      await frames(page, 2)
      await atRest()
      await frames(page, 4)
      const seen = await page.evaluate(() => {
        window.__splitOn = false
        return window.__splitFrames
      })
      if (step.startsWith("⌘D") && (await page.locator(css.pane).count()) === panes)
        throw new CannotRun("no room for a pane beside at this size")
      const rest = await page.evaluate(sample)
      const name = `${step}${reduced ? " (reduced)" : ""}`
      let moved = 0
      for (const part of Object.keys(rest)) {
        const values = seen
          .map((frame) => frame[part])
          .filter((x) => typeof x === "number")
        // A new pane is judged from its first frame; what was there, from before.
        const from = before[part] ?? values[0]
        const to = rest[part]
        if (typeof from !== "number" || Math.abs(from - to) < 2) continue
        moved++
        const between = new Set(
          values
            .filter((x) => x > Math.min(from, to) + 1 && x < Math.max(from, to) - 1)
            .map(Math.round),
        )
        runs.push({ step, reduced, part, from, to, between: between.size })
        if (Math.abs((values.at(-1) ?? to) - to) > 1)
          failures.push(`${name}: ${part} ended at ${values.at(-1)}, rests at ${to}`)
        if (!reduced && between.size < 3)
          failures.push(`${name}: ${part} moved in ${between.size} in-between frames`)
        // With reduced motion nothing travels. A close that gives a folded side
        // column its room back puts it back the frame before the grid draws its
        // new boxes (`paint-with-panes.ts`): one staged frame, not motion.
        if (reduced && between.size > 1)
          failures.push(
            `${name}: ${part} drew ${between.size} in-between frames, not at once`,
          )
        const back = values.slice(1).filter((x, i) => {
          const previous = values[i]
          return to > from ? x < previous - 1 : x > previous + 1
        })
        if (back.length)
          failures.push(
            `${name}: ${part} moved back ${back.length} times: ${values.map(Math.round).join(" ")}`,
          )
      }
      if (moved === 0) failures.push(`${name}: nothing moved`)
    }
    for (const reduced of [false, true]) {
      await page.emulateMedia({ reducedMotion: reduced ? "reduce" : "no-preference" })
      await frames(page, 4)
      // ⌘D splits the chat the caret is in.
      await page.locator(css.composer).first().click()
      await run("⌘D split", keys.splitRight, reduced)
      await run("⌘W close", keys.closePane, reduced)
    }
    await page.emulateMedia({ reducedMotion: "no-preference" })
    return { runs, failures }
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
                // It may slide in from the edge it lands against, never be
                // scaled: its content is drawn at its own size throughout.
                for (const target of [element, element.querySelector(header)]) {
                  if (!target) continue
                  const transform = getComputedStyle(target).transform
                  if (transform === "none") continue
                  const m = new DOMMatrixReadOnly(transform)
                  if (
                    Math.abs(m.a - 1) > 0.001 ||
                    Math.abs(m.d - 1) > 0.001 ||
                    Math.abs(m.b) > 0.001 ||
                    Math.abs(m.c) > 0.001
                  )
                    result.failures.push("mounted pane root or header is scaled")
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
        // A column toggle and a split each move the panes on the drawer's
        // curve, their content painted throughout (above): never dimmed,
        // hidden or rescaled, as #914 asked.
        if (!mounting && result.before === result.after)
          failures.push(`${key}: the toggle changed nothing`)
        else if (!mounting && result.positions < 2)
          failures.push(`${key}: the pane bounds changed nothing`)
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
          {
            titlebar: css.titlebar,
            controls: `${css.titlebar} .workspace-titlebar-navigation .desktop-icon-button, ${css.titlebar} .workspace-titlebar-trailing > .desktop-icon-button`,
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
      return {
        top: bounds.top,
        coversHeader: view === hit || view.contains(hit),
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
    const place = await page.evaluate((title) => {
      const titled = (item) =>
        item.querySelector(".desktop-list-row-label")?.textContent === title
      const rows = [...document.querySelectorAll("[data-session-row]")]
      const listed = rows.find((item) => item.closest(".workspace-list") && titled(item))
      const row = listed ?? rows.find(titled)
      const id = row?.getAttribute("data-session-row")
      return id ? { id, list: listed === row } : null
    }, label)
    if (!place) throw new CannotRun(`no session row titled ${label}`)
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
    const row = place.list
      ? page.locator(`.workspace-list [data-session-row="${place.id}"]`)
      : page.locator(`[data-session-row="${place.id}"]`).first()
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
    const place = await page.evaluate((wanted) => {
      const titled = (item) =>
        item.querySelector(".desktop-list-row-label")?.textContent === wanted
      const rows = [...document.querySelectorAll("[data-session-row]")]
      const listed = rows.find((item) => item.closest(".workspace-list") && titled(item))
      const row = listed ?? rows.find(titled)
      const id = row?.getAttribute("data-session-row")
      return id ? { id, list: listed === row } : null
    }, title)
    if (!place) throw new CannotRun(`no session row titled ${title}`)
    const row = place.list
      ? page.locator(`.workspace-list [data-session-row="${place.id}"]`)
      : page.locator(`[data-session-row="${place.id}"]`).first()
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
