#!/usr/bin/env node
/**
 * Subagents (ADR 329, #330, #331), through the sample source the sample
 * workspace joins: the retry-budget conversation's card opens a pane beside
 * it; the list is in activity order and a closed child is called closed;
 * opening a child shows its transcript, which follows a new line at the end
 * and stays put when scrolled up; focus stays in the panel; Escape returns
 * to the list, and from there a pane's Escape changes nothing more while
 * the window's returns to the panes; the list fits a narrow, short pane.
 *
 * The sample adds three lines to Mara's conversation, at 15s, 25s and 35s
 * after the card is first read (`sample-source.ts`). The follow check waits
 * for two of those.
 *
 * Every check runs on a fresh page, in each engine and layout.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, names } from "./lib/selectors.mjs"
import { contentIs, frames, paneCount, paneCountIs, settled } from "./lib/workspace.mjs"

/** The panel, cropped, when `--shots` is set. Columns only, so the evidence stays small. */
async function shot(page, label, file, selector) {
  if (!label.shots || label.layout !== "columns") return
  mkdirSync(label.shots, { recursive: true })
  const target = page.locator(selector).first()
  if ((await target.count()) === 0) return
  await target.screenshot({ path: join(label.shots, file) })
}

const meta = {
  name: "subagents",
  summary: "subagents panel: list, conversation, follow, Escape, narrow pane",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/subagents.mjs [options]

Checks, per engine and layout (--only <names> to pick):
  panel    the card opens a pane beside the conversation; the list order and
           the closed child; a child opens, focus stays in the panel, the
           transcript follows then stays put when scrolled up; Escape returns
           to the list (a pane's next Escape changes nothing; the window's
           returns to the panes)
  narrow   the list fits a narrow pane in a short window

The panel check waits on the sample's follow-up lines, so it takes about
half a minute.`,
}

/** A fresh page on the sample session whose conversation carries the card. */
async function onSample(browser, { url, layout, width = 1440, height = 900 }) {
  const opened = await openPage(browser, { url, layout, width, height })
  const { page } = opened
  await page.keyboard.press(keys.switcher)
  await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
  await page.keyboard.type(names.subagentsSession)
  await page.keyboard.press(keys.enter)
  await need(page, css.widgetCard, "the subagents card")
  await settled(page)
  return opened
}

async function openPanel(page, failures) {
  const before = await paneCount(page)
  await page
    .locator(css.widgetCard)
    .getByRole("button", { name: names.openWidget })
    .click()
  if (!(await paneCountIs(page, before + 1)))
    failures.push(
      `expected ${before + 1} panes after Open, found ${await paneCount(page)}`,
    )
  await need(page, css.subagentList, "the subagent list")
  await settled(page)
}

function messageCount(page) {
  return page.locator(`${css.subagentMessages} ${css.message}`).count()
}

async function waitForGrowth(page, from, timeout) {
  const grew = await page
    .waitForFunction(
      ([messages, moreThan]) => document.querySelectorAll(messages).length > moreThan,
      [`${css.subagentMessages} ${css.message}`, from],
      { timeout },
    )
    .then(() => true)
    .catch(() => false)
  return grew ? messageCount(page) : from
}

async function scrollGap(page) {
  return page.evaluate((sel) => {
    const node = document.querySelector(sel)
    if (!node) return null
    return {
      top: node.scrollTop,
      height: node.scrollHeight,
      client: node.clientHeight,
      gap: node.scrollHeight - node.scrollTop - node.clientHeight,
    }
  }, css.subagentScroll)
}

const checks = {
  panel: async (page, label) => {
    const failures = []
    await openPanel(page, failures)
    const namesInOrder = await page
      .locator(css.subagentRow)
      .evaluateAll((rows) => rows.map((row) => row.getAttribute("data-subagent-name")))
    if (namesInOrder.join(",") !== "Mara,Idris,Nia,Sol")
      failures.push(`the list is ${namesInOrder.join(", ")}, not Mara, Idris, Nia, Sol`)
    const summary = await page.locator(css.subagentSummary).innerText()
    if (summary !== "1 working · 1 planning · 1 stuck · 1 closed")
      failures.push(`the summary says "${summary}"`)
    const sol = await page
      .locator(`${css.subagentRow}[data-subagent-name="Sol"]`)
      .innerText()
    if (!sol.includes("Closed")) failures.push(`Sol's row says "${sol}", not Closed`)
    if (await page.locator(`${css.subagentPanel} textarea`).count())
      failures.push("the panel has a composer")
    await shot(page, label, `list-${label.engine}.png`, css.subagentPanel)

    await page.locator(`${css.subagentRow}[data-subagent-name="Mara"]`).click()
    await need(page, css.subagentDetail, "Mara's conversation")
    await frames(page)
    const focused = await page.evaluate((detail) => {
      const node = document.querySelector(detail)
      return Boolean(node && node.contains(document.activeElement))
    }, css.subagentDetail)
    if (!focused) failures.push("focus left the open subagent")
    await shot(page, label, `detail-${label.engine}.png`, css.subagentPanel)
    const before = await scrollGap(page)
    if (!before || before.height <= before.client + 40)
      failures.push(`Mara's transcript does not overflow: ${JSON.stringify(before)}`)
    const first = await messageCount(page)
    const followed = await waitForGrowth(page, first, 22_000)
    if (followed === first)
      failures.push("no new line arrived while the transcript was pinned")
    const pinned = await scrollGap(page)
    // Within 40px of the end, the same bound as use-stick-to-bottom.ts.
    if (!pinned || pinned.gap >= 40)
      failures.push(`the transcript did not follow: ${JSON.stringify(pinned)}`)

    await page.locator(css.subagentScroll).evaluate((node) => {
      node.scrollTop = 0
      node.dispatchEvent(new Event("scroll", { bubbles: true }))
    })
    const parked = await messageCount(page)
    const later = await waitForGrowth(page, parked, 16_000)
    if (later === parked) failures.push("no new line arrived after scrolling up")
    const stayed = await scrollGap(page)
    if (!stayed || stayed.top > 8)
      failures.push(`scrolling up did not hold: ${JSON.stringify(stayed)}`)

    const panes = await paneCount(page)
    await page.keyboard.press(keys.escape)
    await settled(page)
    if (!(await page.locator(css.subagentList).count()))
      failures.push("Escape did not return to the list")
    await page.keyboard.press(keys.escape)
    await settled(page)
    if ((await paneCount(page)) !== panes) failures.push("Escape in the pane closed it")
    if (!(await page.locator(css.widgetPane).count()))
      failures.push("the widget pane is gone")

    await page
      .locator(css.widgetPane)
      .getByRole("button", { name: names.openInWindow })
      .click()
    if (!(await contentIs(page, content.widget)))
      failures.push("Open in Window did not show the window")
    await need(page, css.widgetWindow, "the window")
    await page
      .locator(`${css.widgetWindow} ${css.subagentRow}[data-subagent-name="Mara"]`)
      .click()
    await need(page, `${css.widgetWindow} ${css.subagentDetail}`, "Mara in the window")
    await page.keyboard.press(keys.escape)
    await settled(page)
    if (!(await page.locator(`${css.widgetWindow} ${css.subagentList}`).count()))
      failures.push("Escape in the window did not return to the list")
    await page.keyboard.press(keys.escape)
    await settled(page)
    if (!(await contentIs(page, content.panes)))
      failures.push("Escape did not leave the window for the panes")
    return { namesInOrder, summary, failures }
  },

  narrow: async (page, label) => {
    const failures = []
    await openPanel(page, failures)
    await shot(page, label, `narrow-${label.engine}.png`, css.widgetPane)
    const fit = await page.evaluate(
      ([pane, panel]) => {
        const box = document.querySelector(pane)?.getBoundingClientRect()
        const body = document.querySelector(panel)?.getBoundingClientRect()
        return {
          pane: box && { width: box.width, height: box.height },
          panel: body && { width: body.width, height: body.height },
        }
      },
      [css.widgetPane, css.subagentPanel],
    )
    if (!fit.pane || !fit.panel)
      failures.push(`no pane to measure: ${JSON.stringify(fit)}`)
    else {
      if (fit.panel.width > fit.pane.width + 1)
        failures.push(`the panel is wider than its pane: ${JSON.stringify(fit)}`)
      if (fit.panel.height <= 0 || fit.panel.width <= 0)
        failures.push(`the panel has no room: ${JSON.stringify(fit)}`)
    }
    return { fit, failures }
  },
}

const sizes = { narrow: { width: 1000, height: 560 } }

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.list(options.only) : Object.keys(checks)
  for (const name of only)
    if (!Object.hasOwn(checks, name)) throw new CannotRun(`no check named ${name}`)
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const name of only) {
        const size = Object.hasOwn(sizes, name)
          ? sizes[name]
          : { width: 1440, height: 900 }
        let opened
        await attempt(rep, { engine, layout, name }, async () => {
          opened = await onSample(browser, { url, layout, ...size })
          return checks[name](opened.page, {
            engine,
            layout,
            shots: options.shots,
          })
        }).finally(() => opened?.close())
      }
  })
})
