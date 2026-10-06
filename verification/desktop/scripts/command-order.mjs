#!/usr/bin/env node
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { bidiControls } from "../../../src/desktop/workspace/ui/transcript/bidi-controls.mjs"
import { attempt, CannotRun, log, resultOfThrown } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, names } from "./lib/selectors.mjs"
import { contentIs } from "./lib/workspace.mjs"

const tool = "send "
const argument = "\u2028\u202Emoc.live@bob\u202C\u2029"
const expected = { to: argument }

const meta = {
  name: "command-order",
  summary: "an agent's command is drawn in order, its bidi controls visible",
  defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
  help: `
Usage: node verification/desktop/scripts/command-order.mjs [options]

The sample session "${names.commandOrderSession}" (#553). Per engine and layout:
  command   the card, the overview row, and the peek show the argument in the
            order it runs, with no bidi control left to reorder it, and the
            shown JSON parses to the argument that runs
  direction "${names.commandBaseSession}": a tool name written right to left
            stays left of its argument`,
}

const rtlTool = "\u05E9\u05DC\u05D5\u05DD"
const rtlArgument = { to: "bob@example.com" }

function baseOrder(root, expected) {
  const bdi = root.matches("bdi") ? root : root.querySelector("bdi")
  if (!bdi) return "draws no isolated command"
  const direction = getComputedStyle(bdi).direction
  if (direction !== "ltr") return `base direction is ${direction}`
  const text = bdi.textContent ?? ""
  if (!text.startsWith(`${expected.tool} `))
    return `does not lead with the tool: ${JSON.stringify(text)}`
  const jsonAt = text.indexOf("{")
  if (jsonAt <= 0) return "draws no argument after the tool"
  let parsed
  try {
    parsed = JSON.parse(text.slice(jsonAt))
  } catch (error) {
    return `argument does not parse: ${error.message}`
  }
  const keys =
    parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? Object.keys(parsed)
      : []
  const same =
    keys.length === 1 && Object.hasOwn(parsed, "to") && parsed.to === expected.to
  if (!same) return `argument is ${JSON.stringify(parsed)}`
  const words = [...bdi.querySelectorAll(".workspace-approval-word")]
  let toolBox
  let jsonBox
  if (words.length > 1) {
    toolBox = words[0].getBoundingClientRect()
    const jsonWord = words.find((node) => (node.textContent ?? "").includes("{"))
    if (!jsonWord) return "draws no JSON word"
    jsonBox = jsonWord.getBoundingClientRect()
  } else {
    const node = [...bdi.childNodes].find((child) => child.nodeType === Node.TEXT_NODE)
    if (!node || jsonAt > node.textContent.length) return "draws no command text"
    const range = document.createRange()
    range.setStart(node, 0)
    range.setEnd(node, jsonAt)
    toolBox = range.getBoundingClientRect()
    range.setStart(node, jsonAt)
    range.setEnd(node, node.textContent.length)
    jsonBox = range.getBoundingClientRect()
  }
  const box = root.getBoundingClientRect()
  const truncated = root.scrollWidth - root.clientWidth > 1
  if (
    !truncated &&
    toolBox.width >= 1 &&
    jsonBox.width >= 1 &&
    toolBox.left >= jsonBox.left
  )
    return `the tool starts at ${Math.round(toolBox.left - box.left)} and the argument at ${Math.round(jsonBox.left - box.left)}`
  return ""
}

function commandFailures(where, text) {
  const failures = []
  if (!text.startsWith(tool))
    failures.push(
      `${where} does not start with ${JSON.stringify(tool)}: ${JSON.stringify(text)}`,
    )
  if (bidiControls.test(text)) failures.push(`${where} still draws a bidi control`)
  const escapeAt = text.indexOf("\\u202e")
  const addressAt = text.indexOf("moc.live@bob")
  if (escapeAt === -1 || addressAt < escapeAt)
    failures.push(
      `${where} does not show the address after its override: ${JSON.stringify(text)}`,
    )
  if (!text.includes("\\u2028") || !text.includes("\\u2029"))
    failures.push(
      `${where} does not show the line separators: ${JSON.stringify(text)}`,
    )
  if (text.startsWith(tool)) {
    const shown = text.slice(tool.length)
    try {
      const parsed = JSON.parse(shown)
      const keys =
        parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
          ? Object.keys(parsed)
          : []
      const same =
        keys.length === 1 && Object.hasOwn(parsed, "to") && parsed.to === argument
      if (!same)
        failures.push(
          `${where} parses to ${JSON.stringify(parsed)}, not ${JSON.stringify(expected)}`,
        )
    } catch (error) {
      failures.push(`${where} JSON does not parse: ${error.message}`)
    }
  }
  return failures
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const base = { engine, layout }
      let opened
      try {
        opened = await openPage(browser, { url, layout, lines: { engine, layout } })
      } catch (error) {
        rep.add(resultOfThrown({ ...base, name: "command" }, error))
        continue
      }
      const { page } = opened
      await attempt(rep, { ...base, name: "command" }, async () => {
        const failures = []
        await page.keyboard.press(keys.switcher)
        await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
        await page.keyboard.type(names.commandOrderSession)
        await page.keyboard.press(keys.enter)
        const card = page.locator(`${css.focusedPane} ${css.approvalCard}`)
        try {
          await card.first().waitFor({ state: "visible", timeout: 3000 })
        } catch {
          throw new CannotRun(
            `"${names.commandOrderSession}" shows no approval card in the focused pane`,
          )
        }
        const cardText =
          (await card.locator(css.approvalCommand).innerText()).replace(/^\$\s*/, "") ??
          ""
        failures.push(...commandFailures("the card", cardText))
        if (options.shots && failures.length === 0) {
          mkdirSync(options.shots, { recursive: true })
          await card
            .first()
            .screenshot({
              path: join(options.shots, `card-${engine}-${layout}.png`),
            })
            .catch((error) => log(`screenshot card: ${error.message}`))
        }

        await page.keyboard.press(keys.overview)
        await contentIs(page, content.overview)
        const row = page.locator(css.overviewRequest, {
          hasText: names.commandOrderSession,
        })
        if ((await row.count()) === 0)
          failures.push(`the overview lists no row "${names.commandOrderSession}"`)
        else {
          const command = row.locator(css.overviewCommand).first()
          const rowText = (await command.innerText()) ?? ""
          const title = (await command.getAttribute("title")) ?? ""
          failures.push(...commandFailures("the row", rowText))
          if (rowText !== cardText)
            failures.push("the row draws a different command than the card")
          if (title !== rowText)
            failures.push(
              `the row's title is ${JSON.stringify(title)}, not the command it draws`,
            )
          const label = (await row.first().getAttribute("aria-label")) ?? ""
          if (bidiControls.test(label))
            failures.push("the row's accessible name still has a bidi control")
          await row.first().click()
          const peek = page.locator(css.peekCommand)
          let peekText
          try {
            await page.waitForFunction(
              ([sel, expected]) =>
                [...document.querySelectorAll(sel)].some(
                  (node) => (node.innerText ?? "").replace(/^\$\s*/, "") === expected,
                ),
              [css.peekCommand, cardText],
              { timeout: 3000 },
            )
            const drawn = await peek.allInnerTexts()
            peekText =
              drawn
                .map((text) => text.replace(/^\$\s*/, ""))
                .find((text) => text === cardText) ?? ""
          } catch {
            const drawn = await peek.allInnerTexts()
            peekText = (drawn[0] ?? "").replace(/^\$\s*/, "")
          }
          if (!peekText) failures.push("the peek draws no command")
          else {
            failures.push(...commandFailures("the peek", peekText))
            if (peekText !== cardText)
              failures.push("the peek draws a different command than the card")
          }
        }
        if (options.shots && failures.length === 0) {
          mkdirSync(options.shots, { recursive: true })
          await page
            .locator(css.peekCommand, { hasText: "moc.live@bob" })
            .first()
            .screenshot({
              path: join(options.shots, `peek-${engine}-${layout}.png`),
            })
            .catch((error) => log(`screenshot peek: ${error.message}`))
          await row
            .first()
            .screenshot({
              path: join(options.shots, `row-${engine}-${layout}.png`),
            })
            .catch((error) => log(`screenshot row: ${error.message}`))
        }
        await opened.settleRequests()
        return { failures }
      })
      await attempt(rep, { ...base, name: "direction" }, async () => {
        const failures = []
        if (await page.locator(css.overview).isVisible()) {
          await page.keyboard.press(keys.escape)
          if (await page.locator(css.overview).isVisible())
            await page.keyboard.press(keys.escape)
        }
        await page.keyboard.press(keys.switcher)
        await page.waitForSelector(css.switcherField, { state: "visible", timeout: 3000 })
        await page.keyboard.type(names.commandBaseSession)
        await page.keyboard.press(keys.enter)
        const card = page.locator(`${css.focusedPane} ${css.approvalCard}`)
        try {
          await card.first().waitFor({ state: "visible", timeout: 3000 })
        } catch {
          throw new CannotRun(
            `"${names.commandBaseSession}" shows no approval card in the focused pane`,
          )
        }
        const cardOrder = await card
          .locator(css.approvalCommand)
          .evaluate(baseOrder, { tool: rtlTool, to: rtlArgument.to })
        if (cardOrder) failures.push(`the card ${cardOrder}`)
        await page.keyboard.press(keys.overview)
        await contentIs(page, content.overview)
        const row = page.locator(css.overviewRequest, {
          hasText: names.commandBaseSession,
        })
        if ((await row.count()) === 0)
          failures.push(`the overview lists no row "${names.commandBaseSession}"`)
        else {
          const rowOrder = await row
            .locator(css.overviewCommand)
            .first()
            .evaluate(baseOrder, { tool: rtlTool, to: rtlArgument.to })
          if (rowOrder) failures.push(`the row ${rowOrder}`)
          await row.first().click()
          const peek = page.locator(css.peekCommand, { hasText: rtlTool })
          try {
            await peek.first().waitFor({ state: "visible", timeout: 3000 })
          } catch {
            failures.push("the peek draws no command")
          }
          if (await peek.count()) {
            const peekOrder = await peek
              .first()
              .evaluate(baseOrder, { tool: rtlTool, to: rtlArgument.to })
            if (peekOrder) failures.push(`the peek ${peekOrder}`)
          }
        }
        await opened.settleRequests()
        return { failures }
      })
      await opened.close()
    }
  })
})
