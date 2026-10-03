#!/usr/bin/env node
/**
 * An MCP App's review (#436), drawn by the real window over a fake gateway
 * (`fixtures/app-review/`): the card names the app and the tool it asked
 * for, not the agent, and its head stays inside the card at every width,
 * with the tool's name short and very long; the Agents overview's row is
 * named for the app too.
 *
 * The fixture is a page of the dev server's, not of the production build:
 * this script runs in dev mode only.
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { appReview, css, keys } from "./lib/selectors.mjs"
import { frames, settled } from "./lib/workspace.mjs"
import { mkdirSync } from "node:fs"
import { join } from "node:path"

const meta = {
  name: "app-review",
  summary: "an MCP App's review: the card and the overview row name the app",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/app-review.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  card       the head says "The mcptest app wants to run <tool>" and
             data-origin is app; at 280/340/420/600/900 px, with the tool's
             name short and as one very long word, the head stays inside the
             card and the card does not overflow
  overview   the overview row's accessible name names the app and its call`,
}

// What page.evaluate is handed: plain strings (\`css\` holds functions, #441).
const card = { card: css.approvalCard, head: css.approvalHead }

/** The fixture's page, with `tool` the name the app asked for, on the conversation. */
async function onReview(browser, url, layout, tool) {
  const page = new URL(appReview.page, url)
  page.searchParams.set("tool", tool)
  const opened = await openPage(browser, { url: page.href, layout })
  const row = opened.page.locator(css.sessionRow, { hasText: appReview.session }).first()
  await row.waitFor({ timeout: 10_000 }).catch(() => {})
  if (!(await row.count())) {
    await opened.close()
    throw new CannotRun(`no session row "${appReview.session}" in the list`)
  }
  await row.click()
  await need(opened.page, css.approvalCard, "the app's approval card", 10_000)
  await settled(opened.page)
  return opened
}

const checks = {
  async card({ browser, url, layout, engine, options }) {
    const failures = []
    const seen = []
    let said
    for (const [tool, long] of [
      [appReview.tool, false],
      [appReview.longTool, true],
    ]) {
      const opened = await onReview(browser, url, layout, tool)
      const { page } = opened
      try {
        const now = await page.evaluate((sel) => {
          const element = document.querySelector(sel.card)
          return {
            origin: element.dataset.origin ?? null,
            head: element.querySelector(sel.head)?.textContent.trim() ?? null,
          }
        }, card)
        if (!long) said = now
        if (now.origin !== "app")
          failures.push(`${tool}: the card's origin is ${now.origin}, not app`)
        if (now.head !== appReview.head(tool))
          failures.push(`the head says "${now.head}", not "${appReview.head(tool)}"`)
        for (const width of [280, 340, 420, 600, 900]) {
          await page.evaluate(
            ([sel, width]) => {
              let style = document.getElementById("__verify_card")
              if (!style) {
                style = document.createElement("style")
                style.id = "__verify_card"
                document.head.append(style)
              }
              style.textContent = `${sel.card} { width: ${width}px; box-sizing: border-box; }`
            },
            [card, width],
          )
          // The container queries apply in the next frames' style and layout.
          await frames(page, 2)
          const r = await page.evaluate((sel) => {
            const element = document.querySelector(sel.card)
            const box = element.getBoundingClientRect()
            const inner =
              box.right - (parseFloat(getComputedStyle(element).paddingRight) || 0)
            const words = element
              .querySelector(`${sel.head} span`)
              .getBoundingClientRect()
            return {
              card: Math.round(box.width),
              headPastCard: Math.max(0, Math.round(words.right - inner)),
              overflow: element.scrollWidth > element.clientWidth + 1,
            }
          }, card)
          const tag = `${width}px${long ? " long tool" : ""}`
          seen.push({ width, long, ...r })
          if (r.headPastCard)
            failures.push(`${tag}: the head runs ${r.headPastCard}px past the card`)
          if (r.overflow) failures.push(`${tag}: the card overflows`)
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page
              .locator(css.approvalCard)
              .first()
              .screenshot({
                path: join(
                  options.shots,
                  `app-review-${engine}-${layout}-${long ? "long-" : ""}${width}.png`,
                ),
              })
          }
        }
        failures.push(...opened.errors)
      } finally {
        await opened.close()
      }
    }
    return { measured: said, widths: seen, failures }
  },

  async overview({ browser, url, layout }) {
    const opened = await onReview(browser, url, layout, appReview.tool)
    const { page } = opened
    try {
      const failures = []
      await page.keyboard.press(keys.overview)
      await need(page, css.overview, "the Agents overview")
      const name = await page
        .locator(`${css.overviewItem}[data-overview-item="${appReview.sessionId}"]`)
        .first()
        .getAttribute("aria-label", { timeout: 5000 })
        .catch(() => null)
      if (name !== appReview.row(appReview.tool))
        failures.push(
          `the overview row is named "${name}", not "${appReview.row(appReview.tool)}"`,
        )
      return { measured: { rowName: name }, failures: [...failures, ...opened.errors] }
    } finally {
      await opened.close()
    }
  },
}

await main(meta, async ({ options, rep, url, mode }) => {
  if (mode === "prod")
    throw new CannotRun(
      "the app-review fixture is a dev server page: run with --mode dev",
    )
  const only = options.only ? options.only.split(",") : null
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const [name, check] of Object.entries(checks)) {
        if (only && !only.includes(name)) continue
        await attempt(rep, { name, engine, layout }, () =>
          check({ browser, url, layout, engine, options }),
        )
      }
  })
})
