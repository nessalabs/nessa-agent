#!/usr/bin/env node
/**
 * A conversation that opened even though one of its saved records could not
 * be read (#716). Both surfaces show the line "Couldn't read this part of
 * the conversation" and the turns around it. The line carries the record's
 * id and an empty `data-unreadable-report`, where a future report action
 * attaches. It is not the notice for a conversation the window could not
 * open, and not the notice that older history was left out.
 *
 * The fixture is a page of the dev server's.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { transcriptUnreadable as ui } from "./lib/selectors.mjs"

const meta = {
  name: "transcript-unreadable",
  summary:
    "a record that could not be read is one line, and the rest of the conversation stays",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/transcript-unreadable.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  line       on both surfaces, the lines and what the turn says read, top to
             bottom: 3, Plan the release., Here is the plan., 9. Each line
             says "Couldn't read this part of the conversation", carries
             data-unreadable-report, and contains no button. The empty
             welcome is absent, and so are the whole-chat unread sentence
             and the omitted-history sentence
`,
}

const shownWithinMs = 15_000
const wholeChat = "Nessa couldn’t read this conversation just now."
const omitted = "Some conversation content is not shown."

async function open(browser, url, layout) {
  const opened = await openPage(browser, {
    url: new URL(ui.page, url).href,
    layout,
    width: 1280,
    height: 900,
    readySelector: `${ui.panel} ${ui.line("9")}`,
  })
  await opened.page
    .locator(`${ui.desktop} ${ui.line("9")}`)
    .waitFor({ timeout: shownWithinMs })
  return opened
}

const checks = {
  async line({ browser, url, layout, engine, options }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const expected = await page.evaluate(() => window.__unreadable)
      const seen = {}
      for (const surface of [ui.desktop, ui.panel]) {
        const order = await page.evaluate(
          ({ surface, anyLine, said }) =>
            [...document.querySelectorAll(`${surface} :is(${anyLine}, ${said})`)].map(
              (element) =>
                element.hasAttribute("data-unreadable")
                  ? element.getAttribute("data-unreadable")
                  : (element.textContent ?? "").trim(),
            ),
          { surface, anyLine: ui.anyLine, said: ui.said },
        )
        seen[surface] = order
        if (JSON.stringify(order) !== JSON.stringify(expected.order))
          failures.push(
            `${surface} reads ${order.join(" | ")}, not ${expected.order.join(" | ")}`,
          )
        const lines = await page.evaluate(
          ({ surface, anyLine }) =>
            [...document.querySelectorAll(`${surface} ${anyLine}`)].map((element) => ({
              text: element.textContent ?? "",
              report: element.getAttribute("data-unreadable-report"),
              button: Boolean(element.querySelector("button")),
            })),
          { surface, anyLine: ui.anyLine },
        )
        if (lines.length !== 2)
          failures.push(`${surface} has ${lines.length} lines, not 2`)
        for (const line of lines) {
          if (line.text !== expected.sentence)
            failures.push(`${surface} says "${line.text}", not "${expected.sentence}"`)
          if (line.report !== "")
            failures.push(`${surface} report slot is ${JSON.stringify(line.report)}`)
          if (line.button) failures.push(`${surface} line contains a button`)
        }
      }
      const text = await page.locator("body").innerText()
      if (text.includes("Nothing here yet"))
        failures.push("the empty welcome is shown beside the line")
      if (text.includes(wholeChat))
        failures.push("the whole-chat unread sentence is shown")
      if (text.includes(omitted)) failures.push("the omitted-history sentence is shown")
      if (options.shots) {
        mkdirSync(options.shots, { recursive: true })
        const region = (surface) =>
          page.evaluate(
            ({ surface, anyLine, said }) => {
              const boxes = [
                ...document.querySelectorAll(`${surface} :is(${anyLine}, ${said})`),
              ]
                .map((node) => node.getBoundingClientRect())
                .filter((box) => box.width > 0 && box.height > 0)
              if (boxes.length === 0) return null
              const left = Math.min(...boxes.map((box) => box.left))
              const top = Math.min(...boxes.map((box) => box.top))
              const right = Math.max(...boxes.map((box) => box.right))
              const bottom = Math.max(...boxes.map((box) => box.bottom))
              return { x: left, y: top, width: right - left, height: bottom - top }
            },
            { surface, anyLine: ui.anyLine, said: ui.said },
          )
        const desktop = await region(ui.desktop)
        if (!desktop) failures.push(`${ui.desktop} has no line to photograph`)
        else {
          const pad = 28
          await page.screenshot({
            path: join(
              options.shots,
              `transcript-unreadable-desktop-${engine}-${layout}.png`,
            ),
            clip: {
              x: Math.max(0, desktop.x - pad),
              y: Math.max(0, desktop.y - pad),
              width: Math.ceil(desktop.width + pad * 2),
              height: Math.ceil(desktop.height + pad * 2),
            },
          })
        }
        // The panel sits below the window, so an element shot is what shows it.
        await page.locator(ui.panel).screenshot({
          path: join(
            options.shots,
            `transcript-unreadable-panel-${engine}-${layout}.png`,
          ),
        })
      }
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { order: seen, failures }
    } finally {
      await opened.close()
    }
  },
}

await main(meta, async ({ options, rep, url }) => {
  const page = await fetch(new URL(ui.page, url)).then(
    (response) => (response.ok ? response.text() : ""),
    () => "",
  )
  if (!page.includes(`<title>${ui.title}</title>`))
    throw new CannotRun(
      `${ui.page} is not served at ${url}: the fixture is the dev server's (--mode dev)`,
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
