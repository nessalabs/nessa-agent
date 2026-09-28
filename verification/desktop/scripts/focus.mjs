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
 * click on a pane's card (focus-answers-overview, focus-answers-card).
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
    [
      "⌘0 opens",
      (p) => p.keyboard.press(keys.overview),
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
        await settled(p)
        await p.keyboard.press(keys.focusPane(1))
      },
      backInComposer,
    ],
    [
      "⇧⌘[ at the first pane, from the overview",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await settled(p)
        await p.keyboard.press(keys.focusPrevious)
      },
      backInComposer,
    ],
    [
      "⌃⌥← at the left edge, from the overview",
      async (p) => {
        await p.keyboard.press(keys.overview)
        await settled(p)
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

/** How many requests the overview lists now: opened, counted, left. */
async function requestsNow(page) {
  await page.keyboard.press(keys.overview)
  await contentIs(page, content.overview)
  await settled(page)
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
      await card.getByRole("button", { name: names.allowOnce }).focus()
      for (let i = 0; i < 5; i++) await page.keyboard.down(keys.enter)
      await page.keyboard.up(keys.enter)
    } else await card.getByRole("button", { name, exact: true }).dblclick()
    // Counted in the overview once the answer has had its effect, and for a
    // while after, in case a second one was taken.
    await page.keyboard.press(keys.overview)
    await contentIs(page, content.overview)
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
  ⌘0, ↓, ⌘0 again → the caret is on an overview item, and ↓ moves it
  Escape → back to the panes, caret in the focused pane's composer
  from the overview: ⌘1 on the focused pane, ⇧⌘[ at the first → back to the
  panes, caret in the composer; ⌃⌥← at the edge → the overview stays
  focus-answers-overview, -card: a held ⌘↩, and ⌘↩ twice 80ms apart, each answer one request;
  a held ↩ on a pane card's Allow Once, and a double click on Deny and on
  Always Allow, each answer one, and the held ↩ sends nothing typed`,
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
            const failures = [...opened.errors]
            if (!(await paneCount(page))) failures.push("no panes after the burst")
            if (!s.focusedPane) failures.push("no focused pane after the burst")
            return { after: s, failures }
          })
      } finally {
        await opened.close()
      }
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
    }
  })
})
