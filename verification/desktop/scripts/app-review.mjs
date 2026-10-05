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
 * An app's message (#390) is read and drawn the same way: its card says the
 * app wants to send a message as the person — not a tool to run — and shows
 * the message whole; allowed, the message lands labelled with the app that
 * wrote it; denied, the app's request is refused and nothing lands. A
 * context asks nobody and starts no read. With a message as long as the
 * gateway takes, of words or of one unbroken word, the card's head stays
 * inside the card at the same five widths, the card does not overflow, and
 * its answers stay reachable.
 *
 * The fixture is a page of the dev server's, not of the production build:
 * this script runs against the dev server only.
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { appReview, css, keys, offeredLabel } from "./lib/selectors.mjs"
import { frames, settled, unofferedAnswers, until } from "./lib/workspace.mjs"
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
             data-origin app; the review's Allow sends one answer, Allow
             for that review, the card offers only the review's options
             (no Always Allow), the card goes, the app's call is answered
             ok, and the reads stop
  card       at 280/340/420/600/900 px, with the tool's name short and as one
             word as long as the gateway allows, the head stays inside the
             card and the card does not overflow
  overview   the overview row's accessible name names the app and its call
  message    (#390) a context is taken at once and starts no read; the app's
             message is read each round until its review is drawn, its head
             "The mcptest app wants to send a message as you", data-ask
             message, its command the app's tool and the message; Allow Once
             answers that review, the app's request is answered ok, and the
             message lands labelled "Sent by show_rows, from mcptest" over its
             bubble's right edge; the reads stop; a second message denied is
             refused and lands nothing
  message-card
             (#390) with a message as long as the gateway takes — words, then
             one unbroken word — at 280/340/420/600/900 px the head stays inside
             the card, the card does not overflow, and Allow Once is reachable
             (scrolled to, it is what the page hits at its centre)
  message-overview
             (#390) the overview row's accessible name says the app wants to
             send a message as you, the server's name isolated (FSI…PDI)
  message-label
             (#390) a landed message's label, with the app's names short and
             each as long as the gateway allows: in a message column of
             280/340/420/600/900 px, then in an 800×480 window (the desktop's
             least), it is whole — no ellipsis, no title, nothing cut or past
             the column — wrapped onto more lines where it must, each name in
             its own <bdi>, data-message-app server/tool, above its bubble at
             its right edge`,
}

// What page.evaluate is handed: plain strings (`css` holds functions, #441).
const card = {
  card: css.approvalCard,
  head: css.approvalHead,
  headWords: css.approvalHeadWords,
  command: css.approvalCommand,
}

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

/** The app sends `text`; whether the review it asks for is drawn in time. */
async function messaged(page, text) {
  await page.evaluate((text) => window.__appReview.message(text), text)
  const drawn = await until(
    page,
    (sel) => document.querySelector(sel) !== null,
    css.approvalCard,
    drawnWithinMs,
  )
  if (drawn) await settled(page)
  return drawn
}

/** What became of the app's request, once it is no longer waiting (or `waiting` after 5 s). */
async function settledAs(page) {
  await until(
    page,
    () => window.__appReview.snapshot().settled !== "waiting",
    null,
    5_000,
  )
  return (await snapshot(page)).settled
}

const rect = (locator) =>
  locator.evaluate((e) => {
    const r = e.getBoundingClientRect()
    return { x: r.left, y: r.top, w: r.width, h: r.height }
  })

/** The names a label isolates, each in its own `<bdi>`. */
const labelNames = (author) =>
  author
    .evaluate((e) => [...e.querySelectorAll("bdi")].map((name) => name.textContent))
    .catch(() => [])

/**
 * A landed message's label against its column: whether it is whole — every
 * character laid out inside it, none cut by an ellipsis or past its box — on
 * how many lines, and where it sits by its bubble.
 */
const labelFits = (author) =>
  author.evaluate((e) => {
    const message = e.parentElement
    const bubble = message.querySelector(".workspace-bubble")
    const box = e.getBoundingClientRect()
    const column = message.getBoundingClientRect()
    const under = bubble.getBoundingClientRect()
    const style = getComputedStyle(e)
    const line = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2
    // Every character's own box, inside the label's.
    const range = document.createRange()
    range.selectNodeContents(e)
    const glyphs = [...range.getClientRects()]
    const outside = glyphs.filter(
      (r) => r.left < box.left - 0.5 || r.right > box.right + 0.5,
    ).length
    return {
      label: Math.round(box.width),
      column: Math.round(column.width),
      lines: Math.round(box.height / line),
      cut: style.textOverflow === "ellipsis" || style.whiteSpace === "nowrap",
      overflow: e.scrollWidth > e.clientWidth + 1,
      outside,
      pastColumn: Math.max(
        0,
        Math.round(box.right - column.right),
        Math.round(column.left - box.left),
      ),
      above: box.bottom <= under.top + 0.5,
      rightEdgeApart: Math.round(Math.abs(box.right - under.right)),
      title: e.getAttribute("title"),
      app: e.dataset.messageApp ?? null,
    }
  })

/** Sets the person's message column's width, as a pane that wide would. */
async function columnWidth(page, width) {
  await page.evaluate((width) => {
    let style = document.getElementById("__verify_column")
    if (!style) {
      style = document.createElement("style")
      style.id = "__verify_column"
      document.head.append(style)
    }
    style.textContent =
      width === null
        ? ""
        : `.workspace-message[data-role="user"] { width: ${width}px; box-sizing: border-box; }`
  }, width)
  await frames(page, 2)
}

/** What the card says: who asked, by its origin and its head. */
const cardSays = (page) =>
  page.evaluate((sel) => {
    const element = document.querySelector(sel.card)
    return {
      origin: element?.dataset.origin ?? null,
      ask: element?.dataset.ask ?? null,
      head: element?.querySelector(sel.head)?.textContent.trim() ?? null,
      command: element?.querySelector(sel.command)?.textContent ?? null,
    }
  }, card)

/** Sets the card's width, and waits for its container queries to apply. */
async function cardWidth(page, width) {
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
  await frames(page, 2)
}

/** The card's head against its inner edge, and whether the card overflows. */
const cardFits = (page) =>
  page.evaluate((sel) => {
    const element = document.querySelector(sel.card)
    const box = element.getBoundingClientRect()
    const inner = box.right - (parseFloat(getComputedStyle(element).paddingRight) || 0)
    const words = element.querySelector(sel.headWords).getBoundingClientRect()
    return {
      card: Math.round(box.width),
      cardHeight: Math.round(box.height),
      headPastCard: Math.max(0, Math.round(words.right - inner)),
      overflow: element.scrollWidth > element.clientWidth + 1,
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
      // P4: the review's own Allow answers that review; the card goes, the call is answered.
      let answered = waiting
      if (drawn) {
        const card = page.locator(css.approvalCard).first()
        const allow = offeredLabel(waiting.openOptions, "allow")
        failures.push(
          ...(await unofferedAnswers(card, waiting.openOptions)).map(
            (line) => `the card ${line}`,
          ),
        )
        if (!allow) failures.push("the review offers no allow")
        else if (
          (await card.getByRole("button", { name: allow, exact: true }).count()) === 0
        )
          failures.push(`no "${allow}" button on the card`)
        else await card.getByRole("button", { name: allow, exact: true }).click()
        const gone = await until(
          page,
          (sel) => document.querySelector(sel) === null,
          css.approvalCard,
          5_000,
        )
        if (!gone) failures.push(`the card is still drawn 5 s after ${allow ?? "allow"}`)
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
        const bound = await page.evaluate(() => window.__appReview.toolBound)
        const bytes = Buffer.byteLength(tool, "utf8")
        if (long && bytes !== bound)
          failures.push(
            `the long tool's name is ${bytes} UTF-8 bytes, not the ${bound} allowed`,
          )
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
          await cardWidth(page, width)
          const r = await cardFits(page)
          const tag = `${width}px${long ? " long tool" : ""}`
          seen.push({ width, long, toolBytes: bytes, ...r })
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
        // A provider label may be 2,048 characters. It wraps inside the button
        // and stays inside the card; a 64-option harness is more than this check.
        if (!long) {
          const bounded = await page.evaluate((sel) => {
            const element = document.querySelector(sel.card)
            const button = element?.querySelector("button[data-answer]")
            if (!element || !button) return null
            button.textContent = "x".repeat(2048)
            const style = getComputedStyle(button)
            const buttonBox = button.getBoundingClientRect()
            const cardBox = element.getBoundingClientRect()
            return {
              wraps: style.whiteSpace !== "nowrap",
              inside:
                buttonBox.right <= cardBox.right + 1 &&
                element.scrollWidth <= element.clientWidth + 1,
            }
          }, card)
          if (!bounded) failures.push("the card has no answer button to bound")
          else {
            if (!bounded.wraps)
              failures.push("an answer button does not wrap a long label")
            if (!bounded.inside)
              failures.push("a 2048-character label overflows the card")
          }
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page
              .locator(css.approvalCard)
              .first()
              .screenshot({
                path: join(
                  options.shots,
                  `app-review-${engine}-${layout}-long-label.png`,
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
      const waiting = await snapshot(page)
      // Command is Meta on a Mac and Control elsewhere (`commandKey`).
      const mac = await page.evaluate(() => /Mac/.test(navigator.userAgent))
      await page.keyboard.press(mac ? keys.overview : "Control+Digit0")
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
      const row = page
        .locator(`${css.overviewItem}[data-overview-item="${appReview.sessionId}"]`)
        .first()
      if ((await row.count()) > 0) {
        if ((await row.getAttribute("data-offers-always")) !== null)
          failures.push("the overview row offers always, which this review does not")
        failures.push(
          ...(await unofferedAnswers(row, waiting.openOptions)).map(
            (line) => `the overview row ${line}`,
          ),
        )
      }
      return { measured: { rowName: name }, failures: [...failures, ...opened.errors] }
    } finally {
      await opened.close()
    }
  },
}

Object.assign(checks, {
  async message({ browser, url, layout }) {
    const opened = await onConversation(browser, url, layout)
    const { page } = opened
    try {
      const failures = []
      const authors = page.locator(css.messageAuthor)
      if (await authors.count()) failures.push("a label before the app wrote anything")
      // D18: a context is taken at once, asks nobody, and starts no read.
      const rest = await snapshot(page)
      await page.evaluate(() => window.__appReview.context("Showing April"))
      const context = await settledAs(page)
      if (context !== "ok") failures.push(`the context came back ${context}, not ok`)
      await page.waitForTimeout(roundsMs)
      const rested = await snapshot(page)
      if (rested.contexts !== 1)
        failures.push(`the gateway was given ${rested.contexts} contexts, not 1`)
      if (rested.reads !== rest.reads)
        failures.push(`read ${rested.reads - rest.reads} times for a context`)
      if (await page.locator(css.approvalCard).count())
        failures.push("a card is drawn for a context")
      // D9, D19: the message's review is read and drawn as the app's message.
      const askedAt = Date.now()
      const drawn = await messaged(page, appReview.message)
      const drawnMs = Date.now() - askedAt
      if (!drawn) failures.push(notDrawn)
      const said = await cardSays(page)
      const waiting = await snapshot(page)
      const command = `${appReview.appTool} ${JSON.stringify({ text: appReview.message })}`
      if (drawn) {
        if (said.origin !== "app") failures.push(`the card's origin is ${said.origin}`)
        if (said.ask !== "message")
          failures.push(`the card asks ${said.ask}, not message`)
        if (said.head !== appReview.messageHead)
          failures.push(`the head says "${said.head}", not "${appReview.messageHead}"`)
        if (!said.command?.includes(command))
          failures.push(`the command says "${said.command}", not "$ ${command}"`)
      }
      // D1: allowed, the app is answered, and its message lands labelled.
      let answered = waiting
      let landed = {}
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
        const outcome = await settledAs(page)
        answered = await snapshot(page)
        const expected = [[appReview.sessionId, "run", waiting.openReview, "allow"]]
        if (JSON.stringify(answered.answers) !== JSON.stringify(expected))
          failures.push(
            `the gateway was sent ${JSON.stringify(answered.answers)}, not ${JSON.stringify(expected)}`,
          )
        if (outcome !== "ok")
          failures.push(`the app's message came back ${outcome}, not ok`)
        await authors
          .first()
          .waitFor({ timeout: 5000 })
          .catch(() => {})
        const count = await authors.count()
        if (count !== 1) failures.push(`${count} app labels, not 1`)
        const author = authors.first()
        const label = (await author.textContent().catch(() => null)) ?? ""
        const expectedLabel = names.sentBy(appReview.appTool, appReview.server)
        const isolated = await labelNames(author)
        if (
          JSON.stringify(isolated) !==
          JSON.stringify([appReview.appTool, appReview.server])
        )
          failures.push(`the label isolates ${JSON.stringify(isolated)}`)
        if (label !== expectedLabel)
          failures.push(`the label says "${label}", not "${expectedLabel}"`)
        const message = author.locator("xpath=..")
        const bubble = message.locator(css.bubble)
        const words = (await bubble.textContent().catch(() => null)) ?? ""
        if (words !== appReview.message) failures.push(`the bubble says "${words}"`)
        if (count === 1) {
          const labelRect = await rect(author)
          const bubbleRect = await rect(bubble)
          const edge = Math.abs(labelRect.x + labelRect.w - (bubbleRect.x + bubbleRect.w))
          landed = { label, words, rightEdgeApart: edge, labelRect, bubbleRect }
          if (labelRect.y + labelRect.h > bubbleRect.y + 0.5)
            failures.push("the label is not above the bubble")
          if (edge > 6)
            failures.push(`the label is ${edge}px off the bubble's right edge`)
        }
      }
      // The reads stop once it is answered.
      const after = await snapshot(page)
      await page.waitForTimeout(roundsMs)
      const later = await snapshot(page)
      if (later.reads !== after.reads)
        failures.push(
          `read ${later.reads - after.reads} times after the message was answered`,
        )
      // D9: denied, the app's request is refused and nothing lands.
      let denied = null
      if (await messaged(page, "Not this one")) {
        await page.getByRole("button", { name: "Deny", exact: true }).first().click()
        denied = await settledAs(page)
        if (denied !== "refused") failures.push(`a denied message came back ${denied}`)
        await page.waitForTimeout(500)
        if ((await authors.count()) !== 1)
          failures.push("a denied message appeared in the transcript")
      } else failures.push(`second message: ${notDrawn}`)
      return {
        measured: {
          contextReads: rested.reads - rest.reads,
          drawnMs: drawn ? drawnMs : null,
          readsUntilDrawn: waiting.reads - rested.reads,
          readsAfter: later.reads - after.reads,
          answers: answered.answers,
          denied,
          ...said,
          command: said.command?.slice(0, 80),
          landed,
        },
        failures: [...failures, ...opened.errors],
      }
    } finally {
      await opened.close()
    }
  },

  async "message-card"({ browser, url, layout, engine, options }) {
    const failures = []
    const seen = []
    for (const which of ["longestMessage", "longestWord"]) {
      const opened = await onConversation(browser, url, layout)
      const { page } = opened
      try {
        const text = await page.evaluate((which) => window.__appReview[which], which)
        const bound = await page.evaluate(() => window.__appReview.messageBound)
        const bytes = Buffer.byteLength(text, "utf8")
        if (bytes !== bound)
          failures.push(`${which} is ${bytes} UTF-8 bytes, not the ${bound} allowed`)
        if (!(await messaged(page, text))) {
          failures.push(`${which}: ${notDrawn}`)
          continue
        }
        const now = await cardSays(page)
        if (now.head !== appReview.messageHead)
          failures.push(`${which}: the head says "${now.head}"`)
        if (now.ask !== "message") failures.push(`${which}: the card asks ${now.ask}`)
        for (const width of [280, 340, 420, 600, 900]) {
          await cardWidth(page, width)
          const r = await cardFits(page)
          // Its answers stay reachable: scrolled to, Allow Once is what the
          // page hits at its centre.
          const allow = page
            .getByRole("button", { name: names.allowOnce, exact: true })
            .first()
          await allow.scrollIntoViewIfNeeded().catch(() => {})
          await frames(page, 2)
          const reachable = await allow
            .evaluate((button) => {
              const r = button.getBoundingClientRect()
              if (!r.width || !r.height) return false
              const hit = document.elementFromPoint(
                r.left + r.width / 2,
                r.top + r.height / 2,
              )
              return hit !== null && (hit === button || button.contains(hit))
            })
            .catch(() => false)
          const tag = `${width}px ${which}`
          seen.push({ width, which, bytes, reachable, ...r })
          if (r.headPastCard)
            failures.push(`${tag}: the head runs ${r.headPastCard}px past the card`)
          if (r.overflow) failures.push(`${tag}: the card overflows`)
          if (!reachable) failures.push(`${tag}: Allow Once is not reachable`)
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page
              .locator(css.approvalCard)
              .first()
              .screenshot({
                path: join(
                  options.shots,
                  `app-review-message-${engine}-${layout}-${which}-${width}.png`,
                ),
              })
          }
        }
      } finally {
        failures.push(...opened.errors)
        await opened.close()
      }
    }
    return { widths: seen, failures }
  },

  async "message-overview"({ browser, url, layout }) {
    const opened = await onConversation(browser, url, layout)
    const { page } = opened
    try {
      if (!(await messaged(page, appReview.message)))
        return { failures: [notDrawn, ...opened.errors] }
      const failures = []
      await page.keyboard.press(keys.overview)
      await need(page, css.overview, "the Agents overview")
      const name = await page
        .locator(`${css.overviewItem}[data-overview-item="${appReview.sessionId}"]`)
        .first()
        .getAttribute("aria-label", { timeout: 5000 })
        .catch(() => null)
      if (name !== appReview.messageRow)
        failures.push(
          `the overview row is named "${name}", not "${appReview.messageRow}"`,
        )
      return { measured: { rowName: name }, failures: [...failures, ...opened.errors] }
    } finally {
      await opened.close()
    }
  },

  async "message-label"({ browser, url, layout, engine, options }) {
    const failures = []
    const seen = []
    for (const long of [false, true]) {
      const opened = await onConversation(browser, url, layout)
      const { page } = opened
      try {
        const name = long
          ? await page.evaluate(() => window.__appReview.longestTool)
          : null
        const labelled = long
          ? { server: name, tool: name }
          : { server: appReview.server, tool: appReview.appTool }
        await page.evaluate((names) => window.__appReview.labelAs(names), labelled)
        const tag = long ? "long names" : "short names"
        if (!(await messaged(page, appReview.message))) {
          failures.push(`${tag}: ${notDrawn}`)
          continue
        }
        await page
          .getByRole("button", { name: names.allowOnce, exact: true })
          .first()
          .click()
        const author = page.locator(css.messageAuthor).first()
        await author.waitFor({ timeout: 5000 }).catch(() => {})
        if (!(await page.locator(css.messageAuthor).count())) {
          failures.push(`${tag}: no label once the message was allowed`)
          continue
        }
        const expected = names.sentBy(labelled.tool, labelled.server)
        const text = (await author.textContent().catch(() => null)) ?? ""
        if (text !== expected) failures.push(`${tag}: the label says "${text}"`)
        const isolated = await labelNames(author)
        if (JSON.stringify(isolated) !== JSON.stringify([labelled.tool, labelled.server]))
          failures.push(`${tag}: the label isolates ${JSON.stringify(isolated)}`)
        const sizes = [
          ...[280, 340, 420, 600, 900].map((width) => ({ width, window: null })),
          { width: null, window: { width: 800, height: 480 } },
        ]
        for (const size of sizes) {
          if (size.window) await page.setViewportSize(size.window)
          await columnWidth(page, size.width)
          await settled(page)
          const r = await labelFits(author)
          const at = size.window
            ? `${size.window.width}×${size.window.height} window`
            : `${size.width}px column`
          seen.push({ at, long, ...r })
          const where = `${tag}, ${at}`
          if (r.cut) failures.push(`${where}: the label is cut to one line`)
          if (r.overflow) failures.push(`${where}: the label overflows its box`)
          if (r.outside)
            failures.push(`${where}: ${r.outside} glyph boxes outside the label`)
          if (r.pastColumn)
            failures.push(`${where}: the label runs ${r.pastColumn}px past its column`)
          if (!r.above) failures.push(`${where}: the label is not above the bubble`)
          if (r.rightEdgeApart > 6)
            failures.push(
              `${where}: the label is ${r.rightEdgeApart}px off the bubble's right edge`,
            )
          if (r.title !== null) failures.push(`${where}: the label has a title`)
          if (r.app !== `${labelled.server}/${labelled.tool}`)
            failures.push(`${where}: data-message-app is ${r.app}`)
          if (long && size.width === 280 && r.lines < 2)
            failures.push(`${where}: the long label is on ${r.lines} line, not wrapped`)
          if (!long && r.lines !== 1)
            failures.push(`${where}: the short label is on ${r.lines} lines, not 1`)
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await author.locator("xpath=..").screenshot({
              path: join(
                options.shots,
                `app-review-label-${engine}-${layout}-${long ? "long" : "short"}-${size.width ?? "window"}.png`,
              ),
            })
          }
        }
      } finally {
        failures.push(...opened.errors)
        await opened.close()
      }
    }
    return { widths: seen, failures }
  },
})

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
