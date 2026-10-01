#!/usr/bin/env node
/**
 * Widgets (ADR 326), through the sample plugin the sample workspace
 * registers: the trail's card in a message opens a pane beside its
 * conversation, and that pane the window over the panes, and Escape goes
 * back; the window is left by Escape, its close, ⌘W (the pane beneath
 * still there) and a session chosen, and ⌘0 from it goes to the overview;
 * Escape inside a view steps back first; the off and missing cards say so;
 * ⌘1–4 and ⌘W landing on a widget pane put the caret in its body; Escape
 * from a session row leaves the window, and from the search only once its
 * query is cleared; with the edge peek shown, Escape dismisses the peek and
 * nothing more; the trail's detail closing leaves the caret in its pane, or
 * in the window's widget, and a second Escape still reaches the view; the
 * sidebar keeps the channel and the focused session marked beside the
 * window, and the Agents entry unmarked; a session row dragged over the
 * window shows no zone and a release changes nothing; and a widget pane's
 * chrome fits narrow and short panes.
 *
 * Every check runs on a fresh page, in each engine and layout.
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, modules, names } from "./lib/selectors.mjs"
import {
  contentIs,
  frames,
  hideColumns,
  modelRule,
  order,
  paneCount,
  paneCountIs,
  recordZones,
  settled,
  state,
  until,
} from "./lib/workspace.mjs"

const meta = {
  name: "widgets",
  summary: "widget hosts: card, pane, window, Escape, focus, drag, narrow panes",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/widgets.mjs [options]

Checks, per engine and layout (--only <names> to pick):
  card-pane-window   the trail's card opens a pane beside its conversation, the
                     caret in its body; Open in Window shows it over the panes, the
                     caret in it; Escape goes back to the panes as they were
  window-left        the window is left by its close, ⌘W (no pane closed), a
                     session chosen; ⌘0 from it opens the overview
  escape-steps       a detail open in the window or a pane: Escape closes it, the
                     caret staying in the widget; a second Escape still reaches
                     the view (the window: back to the panes)
  off-missing        the off and missing cards say what the hosts say for them
  focus-keys         ⌘1–4 and ⌘W landing on a widget pane put the caret in its body
  escape-row-search  Escape from a session row leaves the window; from the
                     search, it clears the query first and leaves on the next
  escape-peek        with the edge peek shown over the window, Escape dismisses
                     the peek and the window stays
  sidebar-marks      beside the window, the sidebar keeps the channel and the
                     focused session marked, the Agents entry not
  drag-over-window   a session row carried over the window: no zone, no
                     placeholder; the release changes nothing
  host-size          the size a view is told (its host context) is its place's
                     own: a card grown by its content and by a font, a pane
                     resized and split, the window resized
  narrow-short       a widget pane's header and body fit a narrow pane in a short
                     window, and the window's at the same size

off-missing reads the hosts' words from the page's own module, so it needs
--mode dev (the default).`,
}

/** A fresh page on the sample session, whose conversation carries a widget of each state. */
async function onSample(browser, { url, layout, width = 1440, height = 900 }) {
  const opened = await openPage(browser, { url, layout, width, height })
  const { page } = opened
  await page.keyboard.press(keys.switcher)
  await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
  await page.keyboard.type(names.widgetSession)
  await page.keyboard.press(keys.enter)
  await need(page, css.sampleCard, "the sample trail's card")
  await settled(page)
  return opened
}

/** Where the caret is, for the widget checks: its body's host, or what it is. */
const caret = (page) =>
  page.evaluate(
    ([body, pane, win, focused]) => {
      const a = document.activeElement
      if (!a || a === document.body) return "page"
      if (a.matches(body))
        return a.closest(win)
          ? "window body"
          : a.closest(pane)?.matches(focused)
            ? "focused widget pane body"
            : "other widget pane body"
      if (a.tagName === "TEXTAREA" && a.closest(focused)) return "focused composer"
      return `${a.tagName}${a.getAttribute("aria-label") ? `[${a.getAttribute("aria-label")}]` : ""}`
    },
    [css.widgetBody, css.widgetPane, css.widgetWindow, css.focusedPane],
  )

/** Waits until the caret is `where` (see `caret`), and says where it is. */
async function caretAt(page, where) {
  await until(
    page,
    ([body, pane, win, focused, want]) => {
      const a = document.activeElement
      if (!a || !a.matches) return false
      if (want === "window body") return a.matches(body) && !!a.closest(win)
      if (want === "focused widget pane body")
        return a.matches(body) && !!a.closest(pane)?.matches(focused)
      if (want === "focused composer")
        return a.tagName === "TEXTAREA" && !!a.closest(focused)
      return false
    },
    [css.widgetBody, css.widgetPane, css.widgetWindow, css.focusedPane, where],
  )
  return caret(page)
}

const expectCaret = async (page, where, when, failures) => {
  const at = await caretAt(page, where)
  if (at !== where) failures.push(`${when}: the caret is on ${at}, expected the ${where}`)
}

const expectContent = async (page, want, when, failures) => {
  await contentIs(page, want)
  const now = (await state(page)).content
  if (now !== want)
    failures.push(`${when}: the content region shows ${now}, expected ${want}`)
}

/** Clicks the button named `name` inside the first `scope`. */
async function press(page, scope, name) {
  const button = page.locator(scope).first().getByRole("button", { name, exact: false })
  if (!(await button.count())) throw new CannotRun(`no "${name}" button in ${scope}`)
  await button.first().click()
  await settled(page)
}

/** The trail opened beside its conversation from its card: the pane's rect beside the origin's. */
async function openTrailPane(page, failures) {
  const before = await paneCount(page)
  await press(page, css.sampleCard, names.openWidget)
  await paneCountIs(page, before + 1)
  await need(page, css.widgetPane, "the trail's pane")
  return page
    .evaluate(
      ([pane, widget]) => {
        const rect = (e) => {
          const r = e.getBoundingClientRect()
          return { x: r.left, y: r.top, w: r.width, h: r.height }
        }
        const panes = [...document.querySelectorAll(pane)]
        const shown = panes.find((e) => e.matches(widget))
        const origin = panes.find(
          (e) => !e.matches(widget) && e.querySelector("[data-sample-card]"),
        )
        return {
          panes: panes.length,
          widget: shown ? rect(shown) : null,
          origin: origin ? rect(origin) : null,
        }
      },
      [css.pane, css.widgetPane],
    )
    .then((rects) => {
      if (rects.panes !== before + 1)
        failures.push(`the card opened ${rects.panes - before} panes, expected one`)
      if (!rects.widget || !rects.origin)
        failures.push("the trail's pane, or its conversation's, is not on the page")
      else if (
        !(rects.widget.x >= rects.origin.x + rects.origin.w - 1) &&
        !(rects.widget.y >= rects.origin.y + rects.origin.h - 1)
      )
        failures.push(
          `the trail's pane is not beside its conversation: ${JSON.stringify(rects)}`,
        )
      return rects
    })
}

/** Shows the trail in the window from its pane's header. */
async function windowFromPane(page, failures) {
  await page.locator(css.widgetPane).first().hover()
  await press(page, css.widgetPane, names.openInWindow)
  await expectContent(page, content.widget, "Open in Window", failures)
  await need(page, css.widgetWindow, "the window")
}

/** Shows the trail in the window from its card, the session's pane staying focused. */
async function windowFromCard(page, failures) {
  await press(page, css.sampleCard, names.openInWindow)
  await expectContent(page, content.widget, "the card's Open in Window", failures)
  await need(page, css.widgetWindow, "the window")
}

const escape = async (page) => {
  await page.keyboard.press(keys.escape)
  await settled(page)
  await frames(page)
}

const checks = {
  "card-pane-window": async (page) => {
    const failures = []
    const rects = await openTrailPane(page, failures)
    await expectCaret(page, "focused widget pane body", "the card's Open", failures)
    const trail = await page
      .locator(`${css.widgetPane} ${css.widgetTrail}`)
      .first()
      .innerText()
    if (!trail.includes(names.widgetSession))
      failures.push(`the pane's trail says "${trail}", not its conversation first`)
    const panes = (await order(page)).join(",")
    await windowFromPane(page, failures)
    await expectCaret(page, "window body", "Open in Window", failures)
    const window = await page.evaluate(
      ([win, chat, list]) => {
        const r = (sel) => document.querySelector(sel)?.getBoundingClientRect()
        const w = r(win)
        const c = r(chat)
        const l = r(list)
        return {
          window: w && { x: w.left, y: w.top, w: w.width, h: w.height },
          chat: c && { x: c.left, y: c.top, w: c.width, h: c.height },
          listRight: l && l.width > 0 ? l.right : null,
        }
      },
      [css.widgetWindow, css.chatArea, css.sessionList],
    )
    if (
      !window.window ||
      !window.chat ||
      window.window.x < window.chat.x - 1 ||
      window.window.x + window.window.w > window.chat.x + window.chat.w + 1
    )
      failures.push(`the window is not within the chat area: ${JSON.stringify(window)}`)
    if (
      window.listRight !== null &&
      window.window &&
      window.window.x < window.listRight - 1
    )
      failures.push(`the window covers the session list: ${JSON.stringify(window)}`)
    await escape(page)
    await expectContent(page, content.panes, "Escape from the window", failures)
    const after = (await order(page)).join(",")
    if (after !== panes)
      failures.push(`the panes changed under the window: ${panes} → ${after}`)
    await expectCaret(
      page,
      "focused widget pane body",
      "Escape from the window",
      failures,
    )
    return { rects, window, failures }
  },

  "window-left": async (page) => {
    const failures = []
    await openTrailPane(page, failures)
    const panes = (await order(page)).join(",")
    await windowFromPane(page, failures)
    await press(page, css.widgetWindow, names.closeWindow)
    await expectContent(page, content.panes, "the window's close", failures)
    await windowFromPane(page, failures)
    await page.keyboard.press(keys.closePane)
    await settled(page)
    await expectContent(page, content.panes, "⌘W over the window", failures)
    const after = (await order(page)).join(",")
    if (after !== panes)
      failures.push(`⌘W over the window closed a pane: ${panes} → ${after}`)
    await windowFromPane(page, failures)
    // A session chosen: another row, from wherever the layout lists it.
    const row = page.locator(`${css.sessionRow}:not([aria-current="page"])`).first()
    if (!(await row.count())) throw new CannotRun("no other session row to choose")
    await row.click()
    await settled(page)
    await expectContent(page, content.panes, "a session chosen", failures)
    await windowFromCard(page, failures)
    await page.keyboard.press(keys.overview)
    await expectContent(page, content.overview, "⌘0 from the window", failures)
    await escape(page)
    await expectContent(page, content.panes, "Escape from the overview", failures)
    return { failures }
  },

  "escape-steps": async (page) => {
    const failures = []
    await openTrailPane(page, failures)
    const pane = `${css.widgetPane} ${css.sampleStep}`
    // In the pane: the detail closes, the caret stays in the pane, and a
    // second detail is still the view's Escape to close.
    for (const round of [1, 2]) {
      await page.locator(pane).first().click()
      await need(page, `${css.widgetPane} ${css.sampleDetail}`, "the trail's detail")
      await escape(page)
      if (await page.locator(`${css.widgetPane} ${css.sampleDetail}`).count())
        failures.push(`Escape ${round} in the pane left its detail open`)
      await expectCaret(
        page,
        "focused widget pane body",
        `the pane's detail closed (${round})`,
        failures,
      )
    }
    const panes = await paneCount(page)
    await escape(page)
    if ((await paneCount(page)) !== panes)
      failures.push("Escape in a widget pane closed it")
    // In the window: the detail closes, the window stays, the caret in it;
    // the next Escape reaches the host, back to the panes.
    await windowFromPane(page, failures)
    await page.locator(`${css.widgetWindow} ${css.sampleStep}`).first().click()
    await need(page, `${css.widgetWindow} ${css.sampleDetail}`, "the window's detail")
    await escape(page)
    if (await page.locator(`${css.widgetWindow} ${css.sampleDetail}`).count())
      failures.push("Escape in the window left its detail open")
    await expectContent(page, content.widget, "Escape with a detail open", failures)
    await expectCaret(page, "window body", "the window's detail closed", failures)
    await escape(page)
    await expectContent(page, content.panes, "the second Escape in the window", failures)
    return { failures }
  },

  "off-missing": async (page) => {
    const failures = []
    const offered = { inline: true, window: true }
    const said = async (kind) =>
      (
        await modelRule(
          page,
          modules.hostTable,
          "hostDraws",
          "inline",
          { registered: true, name: "Sample", state: { kind } },
          offered,
        )
      ).text
    const off = await said("off")
    const missing = await said("missing")
    const cards = await page
      .locator(`${css.widgetCard}[data-widget-inline="line"]`)
      .allInnerTexts()
    for (const line of [off, missing])
      if (!cards.includes(line))
        failures.push(`no card says "${line}": ${JSON.stringify(cards)}`)
    return { cards, failures }
  },

  "focus-keys": async (page) => {
    const failures = []
    await openTrailPane(page, failures)
    const keyed = await order(page)
    const widgetAt = await page.evaluate(
      ([pane, widget, keysInOrder]) =>
        keysInOrder.findIndex((key) =>
          document.querySelector(`${pane}[data-pane-key="${key}"]`)?.matches(widget),
        ),
      [css.pane, css.widgetPane, keyed],
    )
    const sessionAt = widgetAt === 0 ? 1 : 0
    await page.keyboard.press(keys.focusPane(sessionAt + 1))
    await expectCaret(
      page,
      "focused composer",
      `⌘${sessionAt + 1} on the conversation`,
      failures,
    )
    await page.keyboard.press(keys.focusPane(widgetAt + 1))
    await expectCaret(
      page,
      "focused widget pane body",
      `⌘${widgetAt + 1} on the widget`,
      failures,
    )
    // ⌘W on the conversation: the widget's pane takes focus, the caret in its body.
    await page.keyboard.press(keys.focusPane(sessionAt + 1))
    await expectCaret(page, "focused composer", "back on the conversation", failures)
    await page.keyboard.press(keys.closePane)
    await paneCountIs(page, keyed.length - 1)
    await expectCaret(
      page,
      "focused widget pane body",
      "⌘W landing on the widget",
      failures,
    )
    return { failures }
  },

  "escape-row-search": async (page, layout) => {
    const failures = []
    await windowFromCard(page, failures)
    const row = page.locator(css.sessionRow).first()
    await row.focus()
    await escape(page)
    await expectContent(page, content.panes, "Escape from a session row", failures)
    if (layout !== "columns") return { failures }
    await windowFromCard(page, failures)
    const search = page.locator(`${css.listSearch} input`)
    await search.click()
    await search.fill("widget")
    await escape(page)
    const query = await search.inputValue()
    if (query !== "") failures.push(`Escape left the search's query "${query}"`)
    await expectContent(page, content.widget, "Escape clearing the search", failures)
    await escape(page)
    await expectContent(page, content.panes, "Escape from the empty search", failures)
    return { failures }
  },

  "escape-peek": async (page, layout, size) => {
    const failures = []
    await windowFromCard(page, failures)
    await hideColumns(page, layout)
    await page.mouse.move(3, size.height / 2)
    const peeked = await until(
      page,
      (sel) => document.querySelector(sel)?.hasAttribute("data-peek") === true,
      css.workspace,
    )
    if (!peeked)
      throw new CannotRun("the edge peek did not show from the window's left edge")
    await page.keyboard.press(keys.escape)
    await frames(page, 4)
    const gone = await until(
      page,
      (sel) => !document.querySelector(sel)?.hasAttribute("data-peek"),
      css.workspace,
    )
    if (!gone) failures.push("Escape did not dismiss the edge peek")
    await expectContent(page, content.widget, "Escape dismissing the peek", failures)
    return { failures }
  },

  "sidebar-marks": async (page) => {
    const failures = []
    const marks = () =>
      page.evaluate(
        ([sidebar, list, entry]) => ({
          sidebar: [...document.querySelectorAll(`${sidebar} [aria-current="page"]`)].map(
            (e) => e.getAttribute("data-session-row") ?? e.textContent.trim(),
          ),
          list: [...document.querySelectorAll(`${list} [aria-selected="true"]`)].map(
            (e) => e.getAttribute("data-session-row"),
          ),
          agents: document.querySelector(entry)?.getAttribute("aria-current") ?? null,
        }),
        [css.sidebar, css.sessionList, css.overviewEntry],
      )
    const before = await marks()
    if (!before.sidebar.length)
      throw new CannotRun("nothing is marked in the sidebar before the window")
    await windowFromCard(page, failures)
    const beside = await marks()
    if (JSON.stringify(beside.sidebar) !== JSON.stringify(before.sidebar))
      failures.push(
        `the sidebar's marks moved beside the window: ${JSON.stringify(before.sidebar)} → ${JSON.stringify(beside.sidebar)}`,
      )
    if (JSON.stringify(beside.list) !== JSON.stringify(before.list))
      failures.push(
        `the list's marks moved beside the window: ${JSON.stringify(before.list)} → ${JSON.stringify(beside.list)}`,
      )
    if (beside.agents !== null)
      failures.push(`the Agents entry is marked beside the window (${beside.agents})`)
    return { before, beside, failures }
  },

  "drag-over-window": async (page) => {
    const failures = []
    await openTrailPane(page, failures)
    await windowFromPane(page, failures)
    const before = (await order(page)).join(",")
    const row = page
      .locator(`${css.sidebar} ${css.sessionRow}, ${css.sessionList} ${css.sessionRow}`)
      .last()
    const box = await row.boundingBox()
    if (!box) throw new CannotRun(`no session row to carry (${css.sessionRow})`)
    const over = await page.locator(css.widgetWindow).boundingBox()
    await page.mouse.move(box.x + 40, box.y + box.height / 2)
    await page.mouse.down()
    for (let i = 1; i <= 6; i++)
      await page.mouse.move(box.x + 40 + i * 5, box.y + box.height / 2 + i * 2)
    const zones = await recordZones(page)
    await page.mouse.move(over.x + over.width * 0.6, over.y + over.height / 2, {
      steps: 20,
    })
    // Past `restAfter`, so a zone would have settled if one were offered.
    await page.waitForTimeout(300)
    const said = await zones.take()
    const placeholders = await page.locator(css.dragPlaceholder).count()
    await page.mouse.up()
    await settled(page)
    if (said.length)
      failures.push(`a zone was offered over the window: ${said.join(" → ")}`)
    if (placeholders) failures.push("a placeholder was drawn over the window")
    const after = (await order(page)).join(",")
    if (after !== before)
      failures.push(`the release over the window changed the panes: ${before} → ${after}`)
    await expectContent(page, content.widget, "a release over the window", failures)
    return { failures }
  },

  "host-size": async (page) => {
    const failures = []
    const steps = []
    // What the view was told (`HostContext.size`) against the place's own
    // content box, measured: they agree within a pixel once the page settles.
    const agrees = async (scope, place, when) => {
      const held = await until(
        page,
        ([scopeSel, placeSel, sizeSel]) => {
          for (const root of document.querySelectorAll(scopeSel)) {
            const at = root.matches(placeSel) ? root : root.querySelector(placeSel)
            const told = root.querySelector(sizeSel)?.dataset.sampleSize
            // Only the sample plugin's own views say what they were told.
            if (told === undefined) continue
            if (!at || !told) return false
            const [w, h] = told.split("x").map(Number)
            const style = getComputedStyle(at)
            const box = at.getBoundingClientRect()
            const width =
              box.width -
              parseFloat(style.paddingLeft) -
              parseFloat(style.paddingRight) -
              parseFloat(style.borderLeftWidth) -
              parseFloat(style.borderRightWidth)
            const height =
              box.height -
              parseFloat(style.paddingTop) -
              parseFloat(style.paddingBottom) -
              parseFloat(style.borderTopWidth) -
              parseFloat(style.borderBottomWidth)
            if (Math.abs(w - width) > 1 || Math.abs(h - height) > 1) return false
          }
          return true
        },
        [scope, place, css.sampleSize],
        2000,
      )
      const told = await page
        .locator(`${scope} ${css.sampleSize}`)
        .first()
        .getAttribute("data-sample-size")
      steps.push({ when, told })
      if (!held)
        failures.push(`${when}: the size the view was told (${told}) is not its place's`)
    }
    // A card in a message: its size changed by its content alone, and by a font.
    await agrees(css.widgetCard, css.widgetCard, "the card, laid out")
    await page.addStyleTag({ content: `${css.sampleCard} { padding-block: 24px; }` })
    await agrees(css.widgetCard, css.widgetCard, "the card, grown by its content")
    await page.addStyleTag({ content: `${css.sampleCard} { font-size: 22px; }` })
    await agrees(css.widgetCard, css.widgetCard, "the card, grown by its font")
    // A pane: laid out, the window resized, the pane narrowed by a split.
    await openTrailPane(page, failures)
    await agrees(css.widgetPane, css.widgetBody, "the pane, laid out")
    await page.setViewportSize({ width: 1240, height: 820 })
    await settled(page)
    await agrees(css.widgetPane, css.widgetBody, "the pane, the window resized")
    await page.keyboard.press(keys.focusPane(1))
    await page.keyboard.press(keys.newSessionBeside)
    await settled(page)
    await agrees(css.widgetPane, css.widgetBody, "the pane, narrowed by a split")
    // The window: laid out, and the window resized under it.
    await windowFromPane(page, failures)
    await agrees(css.widgetWindow, css.widgetBody, "the window, laid out")
    await page.setViewportSize({ width: 1440, height: 900 })
    await settled(page)
    await agrees(css.widgetWindow, css.widgetBody, "the window, the window resized")
    return { steps, failures }
  },

  "narrow-short": async (page) => {
    const failures = []
    await openTrailPane(page, failures)
    // Narrower still: a third pane beside.
    await page.keyboard.press(keys.focusPane(1))
    await page.keyboard.press(keys.newSessionBeside)
    await settled(page)
    const measure = (scope) =>
      page.evaluate(
        ([root, header, body, trail, close]) => {
          const at = document.querySelector(root)
          if (!at) return null
          const r = (e) => e?.getBoundingClientRect()
          const box = r(at)
          const inside = r(at.querySelector(body))
          const crumbs = r(at.querySelector(trail))
          const button = r(at.querySelector(close))
          return {
            width: box.width,
            height: box.height,
            headerOverflow:
              at.querySelector(header).scrollWidth - at.querySelector(header).clientWidth,
            trail: crumbs ? crumbs.width : 0,
            close: button ? { left: button.left, right: button.right } : null,
            right: box.right,
            body: inside ? { width: inside.width, height: inside.height } : null,
          }
        },
        [
          scope,
          css.paneHeader,
          css.widgetBody,
          css.widgetTrail,
          `[aria-label^="${scope === css.widgetWindow ? names.closeWindow : names.closePane}"]`,
        ],
      )
    const judge = (what, m) => {
      if (!m) return failures.push(`no ${what} on the page`)
      if (m.headerOverflow > 1)
        failures.push(`${what}'s header overflows by ${m.headerOverflow}px`)
      if (m.trail <= 0) failures.push(`${what}'s trail has no width`)
      if (!m.close || m.close.right > m.right + 1)
        failures.push(`${what}'s close is outside it: ${JSON.stringify(m)}`)
      if (!m.body || m.body.width <= 0 || m.body.height <= 0)
        failures.push(`${what}'s body has no room: ${JSON.stringify(m.body)}`)
    }
    const pane = await measure(css.widgetPane)
    judge("the widget pane", pane)
    await windowFromPane(page, failures)
    const window = await measure(css.widgetWindow)
    judge("the window", window)
    return { pane, window, failures }
  },
}

/** Each check's window size: the narrow and short one is its own. */
const sizes = { "narrow-short": { width: 1000, height: 560 } }

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.list(options.only) : Object.keys(checks)
  for (const name of only)
    if (!Object.hasOwn(checks, name)) throw new CannotRun(`no check named ${name}`)
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const name of only) {
        const size = sizes[name] ?? { width: 1440, height: 900 }
        let opened
        await attempt(rep, { engine, layout, name }, async () => {
          opened = await onSample(browser, { url, layout, ...size })
          const result = await checks[name](opened.page, layout, size)
          const errors = opened.errors
          return { ...result, failures: [...result.failures, ...errors] }
        }).finally(() => opened?.close())
      }
  })
})
