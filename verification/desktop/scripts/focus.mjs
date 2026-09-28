#!/usr/bin/env node
/**
 * Focus follows the focused pane (ADR 238, "Focus follows the focused
 * pane"): whenever another pane takes focus — a split, ⌘N, ⌘W, ⌘1–4,
 * ⇧⌘[ ⇧⌘], a pick in the switcher — the caret lands in its composer.
 * Closing ⌘K without a pick, or Settings, gives focus back to what opened it.
 * In the Agents overview, the keyboard walks its items; leaving it returns
 * the caret to the focused pane's composer.
 */
import { attempt } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys } from "./lib/selectors.mjs"
import {
  focusComposer,
  hideColumns,
  leaveSettings,
  paneCount,
  state,
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
        await p.waitForTimeout(300)
        await p.keyboard.press("ArrowDown")
        await p.keyboard.press("ArrowDown")
        await p.keyboard.press("Enter")
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
        await p.waitForTimeout(300)
        await p.keyboard.press("Escape")
      },
      inComposer,
    ],
    [
      "Settings opened and left",
      async (p) => {
        await p.keyboard.press(keys.settings)
        await p.waitForTimeout(800)
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
  let last = null
  return [
    [
      "⌘0 opens",
      (p) => p.keyboard.press(keys.overview),
      (s) => ((last = s.activeOverviewItem), onItem(s)),
    ],
    [
      "↓ walks",
      (p) => p.keyboard.press("ArrowDown"),
      (s) =>
        onItem(s) ??
        (s.activeOverviewItem !== last
          ? ((last = s.activeOverviewItem), null)
          : "↓ did not move"),
    ],
    ["⌘0 again keeps it", (p) => p.keyboard.press(keys.overview), onItem],
    [
      "Escape leaves",
      (p) => p.keyboard.press("Escape"),
      (s) =>
        s.content === content.panes ? inComposer(s) : `content ${s.content} after Escape`,
    ],
  ]
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
  Escape → back to the panes, caret in the focused pane's composer`,
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
              await page.waitForTimeout(500)
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
              "Escape",
              keys.focusPrevious,
              keys.moveRight,
              keys.focusPane(3),
            ]
            for (let i = 0; i < mash; i++)
              await page.keyboard.press(chords[(i * 7 + (i >> 2)) % chords.length])
            await page.waitForTimeout(1500)
            const s = await state(page)
            const failures = [...opened.errors]
            if (!(await paneCount(page))) failures.push("no panes after the burst")
            if (!s.focusedPane) failures.push("no focused pane after the burst")
            return { after: s, failures }
          })
      } finally {
        await opened.close()
      }
    }
  })
})
