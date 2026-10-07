#!/usr/bin/env node
/**
 * Focus follows the focused pane (ADR 238, "Focus follows the focused
 * pane"): whenever another pane takes focus — a split, ⌘N, ⌘W, ⌘1–4,
 * ⇧⌘[ ⇧⌘], a pick in the switcher — the caret lands in its composer.
 * Closing ⌘K without a pick, or Settings, gives focus back to what opened it.
 * In the Agents overview, the keyboard walks its items; leaving it — by
 * Escape, or a pane command that changed nothing but is a navigation —
 * returns the caret to the focused pane's composer, and a move at the edge
 * leaves the overview where it is. One press answers one request: a held
 * ⌘↩, or two presses 80ms apart, answer one; so does a held ↩ or a double
 * click on a pane's card (focus-answers-overview, focus-answers-card). ⌘R
 * straight after ↓ puts the caret in the reply pill of the row ↓ went to,
 * and what is typed at once lands there; sending, which moves that row from
 * Needs you to Working and draws its pill anew, leaves the caret in the
 * session's pill (focus-reply, at 1440 × 900 and 1000 × 700). The keyboard
 * on a row whose session changes group follows it to its new row, and the
 * arrows walk the list from there — from the peek beneath the row too — while
 * focus the person took to the page stays there (focus-regroup-*). Focus on the
 * scene's Customize control in a new session's home stays there as the window
 * shortens and the home takes a small pane's shape, its header kept as a
 * band (focus-home-scene).
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, names } from "./lib/selectors.mjs"
import {
  contentIs,
  focusComposer,
  frames,
  hideColumns,
  leaveSettings,
  overviewListed,
  paneCount,
  requestCount,
  settled,
  state,
  until,
} from "./lib/workspace.mjs"

const inComposer = (s) =>
  s.activeIsComposer && s.activeInPane === s.focusedPane
    ? null
    : `caret is on ${s.active} in pane ${s.activeInPane ?? "-"}, focused pane is ${s.focusedPane}`

/** Each step: a label, what to do, and what must hold after it. */
function paneSteps() {
  return [
    ["⌘N", (p) => p.keyboard.press(keys.newSession), inComposer],
    [
      "⇧⌘N (new session beside)",
      (p) => p.keyboard.press(keys.newSessionBeside),
      inComposer,
    ],
    [
      "⌘\\ and a pick",
      async (p) => {
        await p.keyboard.press(keys.openBeside)
        await p.waitForSelector(css.switcherField, { state: "visible" })
        await p.keyboard.press(keys.down)
        await p.keyboard.press(keys.down)
        await p.keyboard.press(keys.enter)
      },
      inComposer,
    ],
    ["⇧⌘\\ (split down)", (p) => p.keyboard.press(keys.splitDown), inComposer],
    ["⌘1", (p) => p.keyboard.press(keys.focusPane(1)), inComposer],
    ["⌘2", (p) => p.keyboard.press(keys.focusPane(2)), inComposer],
    ["⌘3", (p) => p.keyboard.press(keys.focusPane(3)), inComposer],
    ["⌘4", (p) => p.keyboard.press(keys.focusPane(4)), inComposer],
    ["⇧⌘]", (p) => p.keyboard.press(keys.focusNext), inComposer],
    ["⇧⌘[", (p) => p.keyboard.press(keys.focusPrevious), inComposer],
    ["⌘W", (p) => p.keyboard.press(keys.closePane), inComposer],
    [
      "⌘K closed without a pick",
      async (p) => {
        await p.keyboard.press(keys.switcher)
        await p.waitForSelector(css.switcherField, { state: "visible" })
        await p.keyboard.press(keys.escape)
      },
      inComposer,
    ],
    [
      "Settings opened and left",
      async (p) => {
        await p.keyboard.press(keys.settings)
        await p.locator(css.settings).first().waitFor({ state: "visible" })
        await settled(p)
        await leaveSettings(p)
      },
      inComposer,
    ],
  ]
}

function overviewSteps() {
  const onItem = (s) =>
    s.content === content.overview && s.activeOverviewItem
      ? null
      : `content ${s.content}, caret on ${s.active}, not an item`
  const backInComposer = (s) =>
    s.content === content.panes
      ? inComposer(s)
      : `content ${s.content}, expected the panes`
  let last = null
  return [
    // Escape leaves whenever the overview is open, before the keyboard has
    // landed on its row too.
    [
      "⌘0 then Escape at once",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await p.keyboard.press(keys.escape)
      },
      backInComposer,
    ],
    [
      "⌘0 opens",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await onOverviewRow(p)
      },
      (s) => ((last = s.activeOverviewItem), onItem(s)),
    ],
    [
      "↓ walks",
      (p) => p.keyboard.press(keys.down),
      (s) =>
        onItem(s) ??
        (s.activeOverviewItem !== last
          ? ((last = s.activeOverviewItem), null)
          : "↓ did not move"),
    ],
    ["⌘0 again keeps it", (p) => p.keyboard.press(keys.overview), onItem],
    [
      "Escape leaves",
      (p) => p.keyboard.press(keys.escape),
      (s) =>
        s.content === content.panes ? inComposer(s) : `content ${s.content} after Escape`,
    ],
    // A pane command from the overview that changes nothing: a navigation
    // leaves, and the caret lands in the focused pane's composer; a move at
    // the edge changes nothing, and the overview keeps the keyboard.
    [
      "⌘ and the focused pane's number, from the overview",
      async (p) => {
        await p.keyboard.press(keys.focusPane(1))
        await p.keyboard.press(keys.overview)
        await onOverviewRow(p)
        await p.keyboard.press(keys.focusPane(1))
      },
      backInComposer,
    ],
    [
      "⇧⌘[ at the first pane, from the overview",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await onOverviewRow(p)
        await p.keyboard.press(keys.focusPrevious)
      },
      backInComposer,
    ],
    [
      "⌃⌥← at the left edge, from the overview",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await onOverviewRow(p)
        await p.keyboard.press(keys.moveLeft)
      },
      onItem,
    ],
    ["Escape leaves again", (p) => p.keyboard.press(keys.escape), backInComposer],
  ]
}

/** Opens a sample session waiting on an approval in the focused pane, by the switcher. */
async function openInPane(page, title) {
  await page.keyboard.press(keys.switcher)
  await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
  await page.keyboard.type(title)
  await page.keyboard.press(keys.enter)
  const card = page.locator(`${css.focusedPane} ${css.approvalCard}`)
  try {
    await card.first().waitFor({ state: "visible", timeout: 3000 })
  } catch {
    throw new CannotRun(`"${title}" shows no approval card in the focused pane`)
  }
  return card.first()
}

/**
 * The caret lands the frame after the current row is drawn. Rows arrive one
 * a frame, so that is later than the peek's animation (`overview.tsx`).
 */
async function onOverviewRow(page) {
  const landed = await until(
    page,
    (sel) => document.activeElement?.closest(sel) != null,
    css.overviewItem,
    1000,
  )
  if (!landed) throw new CannotRun("the keyboard did not land on an overview row")
}

/** How many requests the overview lists now: opened, counted, left. */
async function requestsNow(page) {
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await settled(page)
  if (!(await overviewListed(page)))
    throw new CannotRun("the overview list did not finish drawing")
  const count = await requestCount(page)
  await page.keyboard.press(keys.escape)
  await contentIs(page, content.panes)
  return count
}

/** Waits until the overview lists at most `want` requests; says whether it did. */
const fewer = (page, want) =>
  until(
    page,
    ([sel, n]) => document.querySelectorAll(sel).length <= n,
    [css.overviewRequest, want],
    4000,
  )

/**
 * Whether the overview still lists at least `than` requests after the time a
 * second, wrongly taken answer would take to show — its row's settle and
 * leave. A check that something does not happen needs a window; this is it.
 */
const noFewer = async (page, than) =>
  !(await until(
    page,
    ([sel, n]) => document.querySelectorAll(sel).length < n,
    [css.overviewRequest, than],
    1600,
  ))

/**
 * How far apart the second press comes: well inside the answer pause
 * (`answerPause`, 250ms, src/desktop/workspace/model/overview/walk.ts) — a
 * third of it — so it is a press the person could not yet aim.
 */
const withinAnswerPause = 80

/**
 * One press answers one request in the overview: a held ⌘↩'s repeats answer
 * nothing more, and a second press straight after the keyboard moved on is
 * not taken.
 */
async function answerOnceInOverview(page) {
  const failures = []
  const trail = []

  // The overview: a held ⌘↩, then two presses 80ms apart.
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await settled(page)
  // The list arrives a few rows at a time, and the caret the frame after
  // the current row. Nothing animating is not that landing (`overview.tsx`).
  const onRow = await until(
    page,
    (sel) => document.activeElement?.closest(sel) != null,
    css.overviewItem,
    1000,
  )
  if (!onRow) throw new CannotRun("the keyboard did not land on an overview row")
  if (!(await overviewListed(page)))
    throw new CannotRun("the overview list did not finish drawing")
  const start = await requestCount(page)
  if (start < 2)
    throw new CannotRun(`the overview lists ${start} requests; two are needed`)
  await page.keyboard.down(keys.command)
  for (let i = 0; i < 5; i++) await page.keyboard.down(keys.enter)
  await page.keyboard.up(keys.enter)
  await page.keyboard.up(keys.command)
  const held = (await fewer(page, start - 1)) && (await noFewer(page, start - 1))
  trail.push({ step: "held ⌘↩", before: start, after: await requestCount(page) })
  if (!held)
    failures.push(
      `a held ⌘↩ answered ${start - (await requestCount(page))} requests, not 1`,
    )
  const next = await requestCount(page)
  await page.keyboard.press(keys.allow)
  await page.waitForTimeout(withinAnswerPause)
  await page.keyboard.press(keys.allow)
  const twice = (await fewer(page, next - 1)) && (await noFewer(page, next - 1))
  trail.push({
    step: "⌘↩ twice, 80ms apart",
    before: next,
    after: await requestCount(page),
  })
  if (!twice)
    failures.push(
      `two presses 80ms apart answered ${next - (await requestCount(page))} requests, not 1`,
    )
  await page.keyboard.press(keys.escape)
  await contentIs(page, content.panes)
  return { trail, failures }
}

/**
 * One press answers one request on a pane's card: Allow Once by a held ↩,
 * with words typed and not sent, which the held key must not send; Deny and
 * Always Allow each by a double click.
 */
async function answerOnceOnCard(page) {
  const failures = []
  const trail = []
  for (const [name, answer] of [
    ["Allow Once, held ↩", "held"],
    [names.denyOnce, "double"],
    [names.alwaysAllow, "double"],
  ]) {
    const before = await requestsNow(page)
    let card = null
    for (const candidate of names.approvalSessions) {
      try {
        card = await openInPane(page, candidate)
        break
      } catch {
        // Answered by an earlier step: the next.
      }
    }
    if (!card) throw new CannotRun("no sample session is still waiting on an approval")
    const composer = page.locator(`${css.focusedPane} ${css.field}`).first()
    await composer.fill("keep these words")
    if (answer === "held") {
      // The pick's caret lands in the composer a frame or two after it: let
      // it, then put the keyboard on Allow Once, and hold ↩ only once it is there.
      await settled(page)
      await frames(page, 3)
      const allow = card.getByRole("button", { name: names.allowOnce })
      await allow.focus()
      const onAllow = await until(
        page,
        (name) => document.activeElement?.textContent?.trim() === name,
        names.allowOnce,
        1000,
      )
      if (!onAllow) throw new CannotRun("the keyboard would not stay on Allow Once")
      for (let i = 0; i < 5; i++) await page.keyboard.down(keys.enter)
      await page.keyboard.up(keys.enter)
    } else await card.getByRole("button", { name, exact: true }).dblclick()
    // Counted in the overview once the answer has had its effect, and for a
    // while after, in case a second one was taken.
    await page.keyboard.press(keys.overview)
    await contentIs(page, content.overview)
    // The list is drawn a few rows at a time. An empty prefix is not "fewer".
    if (!(await overviewListed(page)))
      throw new CannotRun("the overview list did not finish drawing")
    const once = (await fewer(page, before - 1)) && (await noFewer(page, before - 1))
    const after = await requestCount(page)
    await page.keyboard.press(keys.escape)
    await contentIs(page, content.panes)
    const typed = await composer.inputValue().catch(() => "")
    trail.push({ step: name, before, after, typed })
    if (!once)
      failures.push(`${name} on a pane's card answered ${before - after} requests, not 1`)
    if (answer === "held" && typed !== "keep these words")
      failures.push(
        `a held ↩ on Allow Once carried into the composer: it now holds "${typed}"`,
      )
  }
  return { trail, failures }
}

/** Where the caret is in the overview: the session whose reply pill holds it, and what it holds. */
const replyCaret = (page) =>
  page.evaluate((field) => {
    const a = document.activeElement
    return a?.matches(field)
      ? {
          session: a.closest("[data-reply-for]")?.dataset.replyFor ?? null,
          value: a.value,
        }
      : { session: null, active: a ? `${a.tagName}.${a.className}` : null }
  }, css.overviewReplyField)

/**
 * ⌘R straight after ↓, then typing at once and sending: the caret is in the
 * pill of the row ↓ went to with every key typed in it, and it stays in that
 * session's pill after the reply moves its row from Needs you to Working.
 */
async function replyKeepsCaret(page) {
  const failures = []
  const trail = []
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await onOverviewRow(page)
  const start = (await state(page)).activeOverviewItem
  // ↓ and ⌘R in one breath, then the words at once: the peek may not have followed ↓ yet.
  await page.keyboard.press(keys.down)
  await page.keyboard.press(keys.reply)
  await page.keyboard.type("status?")
  const typed = await replyCaret(page)
  trail.push({ step: "↓ ⌘R and type", from: start, ...typed })
  const to = typed.session
  if (!to || to === start) {
    failures.push(
      `after ↓ then ⌘R the caret is on ${typed.active ?? to}, not the next row's pill`,
    )
    await page.keyboard.press(keys.escape)
    return { trail, failures }
  }
  if (typed.value !== "status?")
    failures.push(`the pill holds "${typed.value}", not every key typed ("status?")`)
  const moved = await until(
    page,
    ([sel, id]) => document.querySelector(`${sel}[data-overview-item="${id}"]`) !== null,
    [css.overviewRequest, to],
    1000,
  )
  await page.keyboard.press(keys.enter)
  // The reply lets the request go: the row leaves Needs you, its pill drawn anew.
  const left = await until(
    page,
    ([sel, id]) => document.querySelector(`${sel}[data-overview-item="${id}"]`) === null,
    [css.overviewRequest, to],
    4000,
  )
  await frames(page, 3)
  const after = await replyCaret(page)
  trail.push({ step: "sent", wasRequest: moved, rowLeftNeedsYou: left, ...after })
  if (!moved) throw new CannotRun(`the row ↓ reached (${to}) is not a request`)
  if (!left) failures.push(`the row ${to} did not leave Needs you after the reply`)
  if (after.session !== to)
    failures.push(
      `after sending, the caret is on ${after.active ?? after.session}, not ${to}'s pill`,
    )
  else if (after.value !== "")
    failures.push(`after sending, the pill still holds "${after.value}"`)
  await page.keyboard.press(keys.escape)
  return { trail, failures }
}

/** The heading of the group a session's row is listed under, or null where it is not listed. */
const groupOf = (page, sessionId) =>
  page.evaluate(
    ([sel, id]) =>
      document
        .querySelector(
          `${sel.overviewColumn} ${sel.overviewItem}[data-overview-item="${id}"]`,
        )
        ?.closest(sel.overviewGroup)
        ?.querySelector("h2")
        ?.textContent.trim() ?? null,
    [css, sessionId],
  )

/**
 * The keyboard on a row whose session changes group: a reply moves a request
 * to Working, Escape puts the keyboard back on its row, and the scripted turn
 * ending moves it on again, its row drawn anew under another heading (#308).
 * `from` is where the keyboard waits for that: on the row (`row`), where it
 * must follow the session to its new row and the arrows walk the list from
 * there; in the peek opened beneath the row (`peek`, where there is no room
 * beside the list), where it must follow the session to its new row too; or
 * nowhere, the person having clicked the page's title (`away`), where it must
 * stay.
 */
async function regroupKeepsKeyboard(page, from) {
  const failures = []
  const trail = []
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await onOverviewRow(page)
  const on = (await state(page)).activeOverviewItem
  if (!on) throw new CannotRun("the keyboard did not land on an overview row")
  await page.keyboard.press(keys.reply)
  await page.keyboard.type("status?")
  await page.keyboard.press(keys.enter)
  const working = await until(
    page,
    ([sel, id]) =>
      document
        .querySelector(
          `${sel.overviewColumn} ${sel.overviewItem}[data-overview-item="${id}"]`,
        )
        ?.closest(sel.overviewGroup)
        ?.querySelector("h2")
        ?.textContent.trim() === "Working",
    [css, on],
    4000,
  )
  if (!working) throw new CannotRun(`${on} did not move to Working after the reply`)
  await page.keyboard.press(keys.escape)
  await settled(page)
  await frames(page, 3)
  const before = await state(page)
  trail.push({ step: "replied, Escape", under: "Working", ...before })
  if (before.content !== content.overview)
    throw new CannotRun("Escape in the reply pill left the overview")
  if (before.activeOverviewItem !== on)
    throw new CannotRun(
      `after Escape the keyboard is on ${before.active}, not ${on}'s row`,
    )
  if (from === "peek") {
    // The reply opened this session's peek beneath its row; its story takes the keyboard.
    const story = page.locator(
      `${css.overviewItem}[data-overview-item="${on}"] ~ ${css.inlinePeek} ${css.peekStory}`,
    )
    if ((await story.count()) !== 1)
      throw new CannotRun(`no peek is open beneath ${on}'s row`)
    await story.focus()
  } else if (from === "away") {
    // A click on text: the keyboard goes to the page's body.
    await page.locator(`${css.overviewTitle} h1`).click()
  } else if (from === "held") {
    // A press that leaves focus where it is, as the titlebar's drag strip
    // does: a strip that cancels its mousedown, pressed.
    await page.evaluate(() => {
      const strip = document.createElement("div")
      strip.id = "press-keeps-focus"
      strip.style.cssText =
        "position:fixed;left:0;right:0;bottom:0;height:24px;z-index:99999"
      strip.addEventListener("mousedown", (event) => event.preventDefault())
      document.body.append(strip)
    })
    // Pressed, and held until the row has moved on.
    const strip = await page.locator("#press-keeps-focus").boundingBox()
    await page.mouse.move(strip.x + strip.width / 2, strip.y + strip.height / 2)
    await page.mouse.down()
  }
  await frames(page, 2)
  const waiting = await page.evaluate(
    ([story]) => {
      const a = document.activeElement
      return a === document.body ? "body" : a?.matches(story) ? "story" : "row"
    },
    [css.peekStory],
  )
  trail.push({ step: `waiting (${from})`, on: waiting })
  if (waiting !== { row: "row", peek: "story", away: "body", held: "row" }[from])
    throw new CannotRun(`the keyboard waits on ${waiting}, not where ${from} puts it`)
  const moved = await until(
    page,
    ([sel, id]) =>
      document
        .querySelector(
          `${sel.overviewColumn} ${sel.overviewItem}[data-overview-item="${id}"]`,
        )
        ?.closest(sel.overviewGroup)
        ?.querySelector("h2")
        ?.textContent.trim() !== "Working",
    [css, on],
    8000,
  )
  if (!moved) throw new CannotRun(`${on} never left Working: its turn did not end`)
  await frames(page, 3)
  if (from === "held") await page.mouse.up()
  const after = await state(page)
  const under = await groupOf(page, on)
  trail.push({ step: "turn ended", under, ...after })
  if (from === "away") {
    const still = await page.evaluate(() => document.activeElement === document.body)
    if (!still)
      failures.push(
        `${on} moved to ${under ?? "nowhere"} and took the keyboard back to ${after.active}; the person had left it`,
      )
    return { trail, failures }
  }
  if (after.activeOverviewItem !== on)
    failures.push(
      `${on} moved from Working to ${under ?? "nowhere"}; the keyboard is on ${after.active}, not its row`,
    )
  // And the arrows still walk the list: down, or up from the last row.
  await page.keyboard.press(keys.down)
  await frames(page, 3)
  let walked = await state(page)
  if (walked.activeOverviewItem === after.activeOverviewItem) {
    await page.keyboard.press(keys.up)
    await frames(page, 3)
    walked = await state(page)
  }
  trail.push({ step: "↓ (or ↑)", ...walked })
  if (walked.activeOverviewItem === null || walked.activeOverviewItem === on)
    failures.push(`the arrows do not walk the list: the keyboard is on ${walked.active}`)
  return { trail, failures }
}

/** What has focus: an overview row (its session), the page's body, or something else. */
const focused = (page) =>
  page.evaluate((item) => {
    const a = document.activeElement
    if (a === null || a === document.body) return { on: "body" }
    const row = a.matches(item) ? a.dataset.overviewItem : null
    return row ? { on: "row", row } : { on: `${a.tagName}.${a.className}` }
  }, css.overviewItem)

/**
 * Focus the overview loses, it gives back, in cases the regroup checks do
 * not reach (#311 review round 3): `moved`, the focused row's item moved
 * elsewhere in its list in the page, as a list reorders its rows when a
 * session streams past another (the engine's own handling of focus on a
 * move, not a simulation of it); `beside`, focus in the peek beside the list
 * when the window narrows and it goes; `show-all`, Show All from the
 * keyboard, once nothing is left out and it goes. The keyboard must land on
 * a row, and the arrows walk the list from there.
 */
async function givesBackLostFocus(page, cause) {
  const failures = []
  const trail = []
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await onOverviewRow(page)
  if (!(await overviewListed(page)))
    throw new CannotRun("the overview list did not finish drawing")
  let scrolledTo = null
  if (cause === "moved") {
    // Scrolled away from the focused row, as when reading further down.
    await page.setViewportSize({ width: 1440, height: 520 })
    await frames(page, 3)
    scrolledTo = await page.evaluate(() => {
      const scroll = document.querySelector(".agents-overview-scroll")
      if (!scroll || scroll.scrollHeight <= scroll.clientHeight) return null
      scroll.scrollTop = scroll.scrollHeight
      return scroll.scrollTop
    })
    const moved = await page.evaluate((item) => {
      const rows = [...document.querySelectorAll(item)].filter(
        (row) => row.closest("li")?.parentElement?.children.length > 1,
      )
      const row = rows.at(-1)
      if (!row) return null
      row.focus()
      const li = row.closest("li")
      li.parentElement.insertBefore(li, li.parentElement.firstElementChild)
      return row.dataset.overviewItem
    }, css.overviewItem)
    if (!moved) throw new CannotRun("no list holds two rows to move one within")
    trail.push({ step: "moved its row", row: moved })
  } else if (cause === "moved-pill") {
    // Typing a reply beneath a row (no room beside the list), and its row moved.
    await page.setViewportSize({ width: 1000, height: 700 })
    await frames(page, 3)
    await page.keyboard.press(keys.reply)
    await page.keyboard.type("half")
    const moved = await page.evaluate((field) => {
      const typing = document.activeElement
      if (!typing?.matches(field)) return null
      const li = typing.closest("li")
      if (!li || li.parentElement.children.length < 2) return "alone"
      const other = [...li.parentElement.children].find((child) => child !== li)
      li.parentElement.insertBefore(
        li,
        li === li.parentElement.firstElementChild ? null : other,
      )
      return typing.closest("[data-reply-for]")?.dataset.replyFor ?? null
    }, css.overviewReplyField)
    if (moved === null) throw new CannotRun("⌘R did not put the caret in a reply pill")
    if (moved === "alone") throw new CannotRun("the replied-to row is alone in its list")
    await settled(page)
    await frames(page, 3)
    await page.keyboard.type("-way")
    const caret = await replyCaret(page)
    trail.push({ step: "moved the row being replied to", ...caret })
    if (caret.session !== moved || caret.value !== "half-way")
      failures.push(
        `after its row moved, the caret is on ${caret.active ?? caret.session} holding "${caret.value ?? ""}", not ${moved}'s pill holding "half-way"`,
      )
    await page.keyboard.press(keys.escape)
    return { trail, failures }
  } else if (cause === "beside") {
    const peek = page.locator(`${css.overviewPeek} button`).first()
    if ((await peek.count()) === 0)
      throw new CannotRun("no peek beside the list with a control")
    await peek.focus()
    await page.setViewportSize({ width: 700, height: 900 })
    trail.push({ step: "narrowed to 700" })
  } else {
    // The overview's own, not the sidebar's.
    const showAll = page
      .locator(css.overviewColumn)
      .getByRole("button", { name: "Show All" })
    if ((await showAll.count()) === 0)
      throw new CannotRun("the overview leaves nothing out")
    await showAll.focus()
    await page.keyboard.press(keys.enter)
    trail.push({ step: "Show All" })
  }
  await settled(page)
  await frames(page, 3)
  const after = await focused(page)
  trail.push({ step: "after", ...after })
  if (cause === "moved") {
    if (scrolledTo === null) throw new CannotRun("the list does not scroll at 1440 × 520")
    const now = await page.evaluate(
      () => document.querySelector(".agents-overview-scroll")?.scrollTop ?? null,
    )
    trail.push({ step: "scroll", was: scrolledTo, now })
    if (now !== scrolledTo)
      failures.push(`giving focus back scrolled the list from ${scrolledTo} to ${now}`)
  }
  if (after.on !== "row")
    failures.push(`after ${cause}, the keyboard is on ${after.on}, not a row`)
  else {
    await page.keyboard.press(keys.down)
    await frames(page, 3)
    let walked = await focused(page)
    if (walked.row === after.row) {
      await page.keyboard.press(keys.up)
      await frames(page, 3)
      walked = await focused(page)
    }
    trail.push({ step: "↓ (or ↑)", ...walked })
    if (walked.on !== "row" || walked.row === after.row)
      failures.push(`the arrows do not walk the list after ${cause}: ${walked.on}`)
  }
  return { trail, failures }
}

async function firstMessageHandoff(page) {
  const prompt = "one composer through the first message"
  await page.keyboard.press(keys.newSession)
  const ready = await until(
    page,
    ([pane, home, field]) => {
      const current = document.querySelector(pane)
      const input = current?.querySelector(`${home} ${field}`)
      return input != null && document.activeElement === input
    },
    [css.focusedPane, css.paneHome, css.field],
  )
  if (!ready) return { failures: ["new-session home did not receive the caret"] }
  await page.evaluate(
    ([pane, composer]) => {
      const current = document.querySelector(pane)
      const counts = []
      const sample = () => counts.push(current.querySelectorAll(composer).length)
      const observer = new MutationObserver(sample)
      observer.observe(current, { childList: true, subtree: true })
      sample()
      window.__focusHomeHandoff = () => {
        sample()
        observer.disconnect()
        delete window.__focusHomeHandoff
        return counts
      }
    },
    [css.focusedPane, css.composerCard],
  )
  let counts
  const failures = []
  try {
    await page.keyboard.type(prompt)
    await page.keyboard.press(keys.enter)
    const arrived = await until(
      page,
      ([pane, home, dock, field, bubble, text]) => {
        const current = document.querySelector(pane)
        const input = current?.querySelector(`${dock} ${field}`)
        return (
          input != null &&
          !current.querySelector(home) &&
          [...current.querySelectorAll(bubble)].some(
            (part) => part.textContent.trim() === text,
          ) &&
          document.activeElement === input
        )
      },
      [
        css.focusedPane,
        css.paneHome,
        css.conversationDock,
        css.field,
        css.bubble,
        prompt,
      ],
    )
    if (!arrived) failures.push("first message did not arrive with the reply caret")
    else {
      await page.keyboard.type("next reply")
      const value = await page
        .locator(`${css.focusedPane} ${css.conversationDock} ${css.field}`)
        .inputValue()
      if (value !== "next reply") failures.push(`reply draft is ${JSON.stringify(value)}`)
    }
  } finally {
    counts = await page.evaluate(() => window.__focusHomeHandoff?.() ?? [])
  }
  if (counts.length === 0) failures.push("handoff had no observed composer counts")
  if (counts.some((count) => count > 1))
    failures.push(`overlapping composers during handoff: ${counts.join(" ")}`)
  return { counts, after: await state(page), failures }
}

const meta = {
  name: "focus",
  summary:
    "the caret follows the focused pane; dialogs give focus back; the overview walks and returns",
  defaults: { engine: "chromium,webkit" },
  options: { mash: { type: "string", default: "120" } },
  help: `
Usage: node verification/desktop/scripts/focus.mjs [options]

  --mash <n>   Keys pressed in a burst at the end (default 120; 0 to skip);
               afterwards the page must have raised no error and still have
               a focused pane.

Steps (each asserts where the caret is afterwards):
  ⌘N, ⇧⌘N, ⌘\\ + pick, ⇧⌘\\, ⌘1–4, ⇧⌘], ⇧⌘[, ⌘W, ⌘K + Escape, Settings + Back
  → the caret is in the focused pane's composer
  ⌘0 then Escape at once → back to the panes, caret in the composer
  ⌘0, ↓, ⌘0 again → the caret is on an overview item, and ↓ moves it
  Escape → back to the panes, caret in the focused pane's composer
  from the overview: ⌘1 on the focused pane, ⇧⌘[ at the first → back to the
  panes, caret in the composer; ⌃⌥← at the edge → the overview stays
  focus-reply (1440 × 900 and 1000 × 700): ↓ then ⌘R at once, and typing at
  once → the caret and every key are in the next row's reply pill; sending,
  which moves that row to Working → the caret stays in that session's pill
  focus-answers-overview, -card: a held ⌘↩, and ⌘↩ twice 80ms apart, each answer one request;
  a held ↩ on a pane card's Allow Once, and a double click on Deny and on
  Always Allow, each answer one, and the held ↩ sends nothing typed
  focus-regroup-row, -peek, -away: a reply moves a request to Working, Escape
  puts the keyboard on its row, its turn ends and the row moves on → from the
  row (1440 × 900) or from the peek beneath it (1000 × 700), the keyboard is
  on that session's row, and ↓ walks the list from the row; after a click on
  the title (away), it stays on the page's body; with a press held across
  the move that leaves focus on the row (held, a strip that cancels its
  mousedown), it follows the row
  focus-gives-back-moved, -moved-pill, -beside, -show-all: the focused row's
  item moved within its list, the list scrolled away (1440 × 520) and left
  there; a reply being typed beneath a row (1000 × 700) whose row is moved,
  the caret kept in the pill and the next keys in it; focus in the peek beside the list as the window narrows
  to 700; Show All pressed once nothing is left out → the keyboard is on a
  row, and ↓ walks the list
  focus-home-handoff: the first message replaces the home with one composer;
    the reply receives the caret and keeps the next draft.
  focus-home-scene: Customize focused in a new session's home, the window
  shortened so the home takes a small pane's shape → focus stays on Customize`,
}

/**
 * A new session's home alone in the pane shows its scene; the scene's
 * Customize control takes focus; the window shortens, so the pane is under
 * 640px and its home takes a conversation's shape. The header stays, lower,
 * with Customize in it (`conversation.css`, issue #320), so the focus stays
 * where the person put it rather than being moved or lost to the page.
 */
async function sceneKeepsFocus(page) {
  const failures = []
  await page.keyboard.press(keys.newSession)
  await need(page, css.homeCustomize, "the scene's Customize control")
  await settled(page)
  await page.locator(css.homeCustomize).focus()
  const held = await page.evaluate(
    (sel) => document.activeElement?.matches(sel) === true,
    css.homeCustomize,
  )
  if (!held) throw new CannotRun("Customize did not take focus")
  await page.setViewportSize({ width: 1440, height: 600 })
  await settled(page)
  await frames(page, 3)
  const after = await state(page)
  const paneHeight = await page.evaluate(
    ([home, pane]) =>
      document.querySelector(home)?.closest(pane)?.getBoundingClientRect().height ?? 0,
    [css.paneHome, css.pane],
  )
  if (paneHeight >= 640)
    throw new CannotRun(`the home's pane is ${Math.round(paneHeight)}px tall at 600px`)
  const scene = await page.evaluate(
    (sel) => document.querySelector(sel)?.checkVisibility() ?? false,
    css.homeCustomize,
  )
  if (!scene) failures.push("the scene's Customize control is gone in a small pane")
  const onCustomize = await page.evaluate(
    (sel) => document.activeElement?.matches(sel) === true,
    css.homeCustomize,
  )
  if (!onCustomize)
    failures.push(`the home became small: focus moved to ${after.active}, off Customize`)
  return { after, failures }
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const opened = await openPage(browser, { url, layout, width: 1440, height: 900 })
      try {
        await need(opened.page, css.composer, "a composer")
        const page = opened.page
        await hideColumns(page, layout)
        await focusComposer(page)
        for (const [group, steps] of [
          ["panes", paneSteps()],
          ["overview", overviewSteps()],
        ])
          await attempt(rep, { name: `focus-${group}`, engine, layout }, async () => {
            const failures = []
            const trail = []
            for (const [label, act, check] of steps) {
              await act(page)
              await settled(page)
              // The caret lands a frame or two after what moved it.
              await frames(page, 3)
              const s = await state(page)
              const problem = check(s)
              trail.push({ step: label, ...s, ok: !problem })
              if (problem) failures.push(`${label}: ${problem}`)
            }
            return { trail, failures }
          })

        const mash = Number(options.mash)
        if (mash > 0)
          await attempt(rep, { name: "focus-mash", engine, layout }, async () => {
            const chords = [
              keys.newSession,
              keys.closePane,
              keys.focusPane(2),
              keys.newSessionBeside,
              keys.toggleSidebar,
              keys.toggleSessionList,
              keys.focusPane(1),
              keys.closePane,
              keys.switcher,
              keys.escape,
              keys.focusPrevious,
              keys.moveRight,
              keys.focusPane(3),
            ]
            for (let i = 0; i < mash; i++)
              await page.keyboard.press(chords[(i * 7 + (i >> 2)) % chords.length])
            await settled(page, 5000)
            await frames(page, 3)
            const s = await state(page)
            const failures = []
            if (!(await paneCount(page))) failures.push("no panes after the burst")
            if (!s.focusedPane) failures.push("no focused pane after the burst")
            return { after: s, failures }
          })
      } finally {
        await opened.close()
      }
      await attempt(rep, { name: "focus-home-handoff", engine, layout }, async () => {
        const fresh = await openPage(browser, { url, layout, width: 1440, height: 900 })
        try {
          const result = await firstMessageHandoff(fresh.page)
          return result
        } finally {
          await fresh.close()
        }
      })
      for (const [name, answer] of [
        ["focus-answers-overview", answerOnceInOverview],
        ["focus-answers-card", answerOnceOnCard],
      ])
        await attempt(rep, { name, engine, layout }, async () => {
          const fresh = await openPage(browser, { url, layout, width: 1440, height: 900 })
          try {
            await need(fresh.page, css.composer, "a composer")
            await focusComposer(fresh.page)
            const result = await answer(fresh.page)
            return { ...result, failures: [...result.failures, ...fresh.errors] }
          } finally {
            await fresh.close()
          }
        })
      for (const [from, width, height] of [
        ["row", 1440, 900],
        ["peek", 1000, 700],
        ["away", 1440, 900],
        ["held", 1440, 900],
      ])
        await attempt(
          rep,
          { name: `focus-regroup-${from}`, engine, layout, size: `${width}x${height}` },
          async () => {
            const fresh = await openPage(browser, { url, layout, width, height })
            try {
              await need(fresh.page, css.composer, "a composer")
              await focusComposer(fresh.page)
              const result = await regroupKeepsKeyboard(fresh.page, from)
              return { ...result, failures: [...result.failures, ...fresh.errors] }
            } finally {
              await fresh.close()
            }
          },
        )
      for (const cause of ["moved", "moved-pill", "beside", "show-all"])
        await attempt(
          rep,
          { name: `focus-gives-back-${cause}`, engine, layout },
          async () => {
            const fresh = await openPage(browser, {
              url,
              layout,
              width: 1440,
              height: 900,
            })
            try {
              await need(fresh.page, css.composer, "a composer")
              await focusComposer(fresh.page)
              const result = await givesBackLostFocus(fresh.page, cause)
              return { ...result, failures: [...result.failures, ...fresh.errors] }
            } finally {
              await fresh.close()
            }
          },
        )
      await attempt(rep, { name: "focus-home-scene", engine, layout }, async () => {
        const fresh = await openPage(browser, { url, layout, width: 1440, height: 900 })
        try {
          await need(fresh.page, css.composer, "a composer")
          const result = await sceneKeepsFocus(fresh.page)
          return { ...result, failures: [...result.failures, ...fresh.errors] }
        } finally {
          await fresh.close()
        }
      })
      for (const [width, height] of [
        [1440, 900],
        [1000, 700],
      ])
        await attempt(
          rep,
          { name: "focus-reply", engine, layout, size: `${width}x${height}` },
          async () => {
            const fresh = await openPage(browser, { url, layout, width, height })
            try {
              await need(fresh.page, css.composer, "a composer")
              await focusComposer(fresh.page)
              const result = await replyKeepsCaret(fresh.page)
              return { ...result, failures: [...result.failures, ...fresh.errors] }
            } finally {
              await fresh.close()
            }
          },
        )
    }
  })
})
