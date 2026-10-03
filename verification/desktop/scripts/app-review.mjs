#!/usr/bin/env node
/**
 * An MCP App's review (#436), in the window over a fake gateway
 * (`fixtures/app-review/`), whose conversation's turn has ended: at rest it
 * is not read again; when the app calls a destructive tool, the review the
 * gateway opens for it a few rounds later — which moves nothing in the list
 * row — is read and drawn, naming the app and the tool, not the agent;
 * allowed, it goes, the gateway is told Allow for that review, and the
 * app's call is answered. The card's head stays inside the card at five
 * widths from 280 to 900 px, with the tool's name short and as long as the
 * gateway allows; the Agents overview's row is named for the app too.
 *
 * The fixture is a page of the dev server's, not of the production build:
 * this script runs against the dev server only.
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { appReview, css, keys, names } from "./lib/selectors.mjs"
import { frames, settled, until } from "./lib/workspace.mjs"
import { mkdirSync } from "node:fs"
import { join } from "node:path"

const meta = {
  name: "app-review",
  summary: "an MCP App's review: read while its call waits, drawn as the app's, answered",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/app-review.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  review     at rest the conversation is not read again; after the app's
             call it is read each round — the fake opens the review only once
             it has been read twice since the call — and the review is drawn
             within 8 s, its head "The mcptest app wants to run <tool>" and
             data-origin app; Allow Once sends one answer, Allow for that
             review, the card goes, the app's call is answered ok, and the
             reads stop
  card       at 280/340/420/600/900 px, with the tool's name short and as one
             word as long as the gateway allows, the head stays inside the
             card and the card does not overflow
  overview   the overview row's accessible name names the app and its call`,
}

// What page.evaluate is handed: plain strings (`css` holds functions, #441).
const card = { card: css.approvalCard, head: css.approvalHead }

// The source polls each second (`defaultGatewayTiming.pollMs`): two and a
// half rounds would show a read that should not happen.
const roundsMs = 2_500
// Two rounds before the fake opens the review, one to read it, and room.
const drawnWithinMs = 8_000

const snapshot = (page) => page.evaluate(() => window.__appReview.snapshot())

/** The fixture's page, on the conversation, before the app has called anything. */
async function onConversation(browser, url, layout) {
  const opened = await openPage(browser, {
    url: new URL(appReview.page, url).href,
    layout,
  })
  const row = opened.page.locator(css.sessionRow, { hasText: appReview.session }).first()
  await row.waitFor({ timeout: 10_000 }).catch(() => {})
  if (!(await row.count())) {
    await opened.close()
    throw new CannotRun(`no session row "${appReview.session}" in the list`)
  }
  await row.click()
  await settled(opened.page)
  return opened
}

/** The app calls `tool`; whether the review it asks for is drawn in time. */
async function asked(page, tool) {
  await page.evaluate((tool) => window.__appReview.call(tool), tool)
  const drawn = await until(
    page,
    (sel) => document.querySelector(sel) !== null,
    css.approvalCard,
    drawnWithinMs,
  )
  if (drawn) await settled(page)
  return drawn
}

const notDrawn = `no card within ${drawnWithinMs / 1000} s of the app's call`

/** What the card says: who asked, by its origin and its head. */
const cardSays = (page) =>
  page.evaluate((sel) => {
    const element = document.querySelector(sel.card)
    return {
      origin: element?.dataset.origin ?? null,
      head: element?.querySelector(sel.head)?.textContent.trim() ?? null,
    }
  }, card)

const checks = {
  async review({ browser, url, layout }) {
    const opened = await onConversation(browser, url, layout)
    const { page } = opened
    try {
      const failures = []
      // P1: at rest, nothing waits and the conversation is not read again.
      if (await page.locator(css.approvalCard).count())
        failures.push("a card is drawn before the app asked for anything")
      const rest = await snapshot(page)
      await page.waitForTimeout(roundsMs)
      const rested = await snapshot(page)
      if (rested.reads !== rest.reads)
        failures.push(`read ${rested.reads - rest.reads} times at rest`)
      // P2, P3: the app's call; its review is read and drawn as the app's.
      const askedAt = Date.now()
      const drawn = await asked(page, appReview.tool)
      const drawnMs = Date.now() - askedAt
      if (!drawn) failures.push(notDrawn)
      const said = await cardSays(page)
      const waiting = await snapshot(page)
      if (drawn && said.origin !== "app")
        failures.push(`the card's origin is ${said.origin}, not app`)
      if (drawn && said.head !== appReview.head(appReview.tool))
        failures.push(
          `the head says "${said.head}", not "${appReview.head(appReview.tool)}"`,
        )
      // P4: Allow Once answers that review with Allow; the card goes, the call is answered.
      let answered = waiting
      if (drawn) {
        await page
          .getByRole("button", { name: names.allowOnce, exact: true })
          .first()
          .click()
        const gone = await until(
          page,
          (sel) => document.querySelector(sel) === null,
          css.approvalCard,
          5_000,
        )
        if (!gone) failures.push("the card is still drawn 5 s after Allow Once")
        answered = await snapshot(page)
        const expected = [[appReview.sessionId, "run", waiting.openReview, "allow"]]
        if (JSON.stringify(answered.answers) !== JSON.stringify(expected))
          failures.push(
            `the gateway was sent ${JSON.stringify(answered.answers)}, not ${JSON.stringify(expected)}`,
          )
        if (answered.settled !== "ok")
          failures.push(`the app's call came back ${answered.settled}, not ok`)
      }
      // P5: answered, and read without the review, the reads stop.
      const after = await snapshot(page)
      await page.waitForTimeout(roundsMs)
      const later = await snapshot(page)
      if (later.reads !== after.reads)
        failures.push(
          `read ${later.reads - after.reads} times after the call was answered`,
        )
      if (later.answers.length !== after.answers.length)
        failures.push("the gateway was answered again after the card went")
      return {
        measured: {
          drawnMs: drawn ? drawnMs : null,
          readsAtRest: rested.reads - rest.reads,
          readsUntilDrawn: waiting.reads - rested.reads,
          readsAfter: later.reads - after.reads,
          answers: answered.answers,
          settled: answered.settled,
          ...said,
        },
        failures: [...failures, ...opened.errors],
      }
    } finally {
      await opened.close()
    }
  },

  async card({ browser, url, layout, engine, options }) {
    const failures = []
    const seen = []
    let said
    for (const long of [false, true]) {
      const opened = await onConversation(browser, url, layout)
      const { page } = opened
      try {
        const tool = long
          ? await page.evaluate(() => window.__appReview.longestTool)
          : appReview.tool
        if (!(await asked(page, tool))) {
          failures.push(`${long ? "long tool: " : ""}${notDrawn}`)
          continue
        }
        const now = await cardSays(page)
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
          seen.push({ width, long, toolBytes: tool.length, ...r })
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
      } finally {
        failures.push(...opened.errors)
        await opened.close()
      }
    }
    return { measured: said, widths: seen, failures }
  },

  async overview({ browser, url, layout }) {
    const opened = await onConversation(browser, url, layout)
    const { page } = opened
    try {
      if (!(await asked(page, appReview.tool)))
        return { failures: [notDrawn, ...opened.errors] }
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

await main(meta, async ({ options, rep, url }) => {
  // A production build has no such page and answers any path with its own:
  // asked once, by the fixture's title, before any engine starts, whatever
  // --mode or --url said.
  const page = await fetch(new URL(appReview.page, url)).then(
    (response) => (response.ok ? response.text() : ""),
    () => "",
  )
  if (!page.includes(`<title>${appReview.title}</title>`))
    throw new CannotRun(
      `${appReview.page} is not served at ${url}: the fixture is the dev server's (--mode dev)`,
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
