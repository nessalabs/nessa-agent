#!/usr/bin/env node
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { bidiControls } from "../../../src/desktop/workspace/ui/transcript/bidi-controls.mjs"
import { attempt, CannotRun, resultOfThrown } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, names } from "./lib/selectors.mjs"
import { contentIs } from "./lib/workspace.mjs"

const tool = "send "
const argument = "\u202Emoc.live@bob\u202C"
const expected = { to: argument }

const meta = {
  name: "command-order",
  summary: "an agent's command is drawn in order, its bidi controls visible",
  defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
  help: `
Usage: node verification/desktop/scripts/command-order.mjs [options]

The sample session "${names.commandOrderSession}" (#553). Per engine and layout:
  command  the card, the overview row, and the peek show the argument in the
           order it runs, with no bidi control left to reorder it, and the
           shown JSON parses to the argument that runs`,
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
        opened = await openPage(browser, { url, layout })
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
            .catch(() => {})
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
            .catch(() => {})
          await row
            .first()
            .screenshot({
              path: join(options.shots, `row-${engine}-${layout}.png`),
            })
            .catch(() => {})
        }
        await opened.settleRequests()
        return {
          failures: [...failures, ...opened.errors.splice(0)],
          harmless: opened.harmless.splice(0),
        }
      })
      await opened.close()
    }
  })
})
