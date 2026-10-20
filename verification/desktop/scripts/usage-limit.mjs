#!/usr/bin/env node
/** #1075: the usage-limit card on the desktop transcript and the panel. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { usageLimit as selectors } from "./lib/selectors.mjs"

const TITLE = "Usage limit reached"
const EARLY = "Your last turn on this request ended early"
const REQUIRED = "The turn could not complete all required work."

const cases = [
  {
    name: "timed",
    search: "",
    detail: /^Auto-resuming /,
    checked: true,
  },
  {
    name: "unknown",
    search: "case=unknown",
    detail: "Reset time unknown",
    checked: false,
  },
  {
    name: "user-action",
    search: "case=user-action",
    detail: "An account change is required before this thread can continue.",
    checked: true,
  },
  {
    name: "scheduling-failed",
    search: "case=scheduling-failed",
    detail: "Scheduling failed.",
    checked: true,
  },
]

await main(
  {
    name: "usage-limit",
    summary: "usage-limit card: reset copy and the project continuation toggle",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of [360, 1000]) {
        for (const item of cases) {
          await attempt(rep, { name: item.name, engine, width }, async () => {
            const target = new URL(selectors.page, url)
            if (item.search) target.search = item.search
            const opened = await openPage(browser, {
              url: target.href,
              width,
              height: 800,
              readySelector: `${selectors.desktop} ${selectors.card}`,
            })
            const { page } = opened
            try {
              const desktop = page.locator(selectors.desktop)
              const panel = page.locator(selectors.panel)
              await panel.locator(selectors.card).waitFor()
              const details = []
              for (const surface of [desktop, panel]) {
                const card = surface.locator(selectors.card)
                await card.scrollIntoViewIfNeeded()
                const heading = (await card.locator("h3").innerText())
                  .replace(/\s+/g, " ")
                  .trim()
                if (!heading.endsWith(TITLE))
                  throw new Error(`heading changed: ${heading}`)
                const detail = (await card.locator(selectors.detail).innerText()).trim()
                if (typeof item.detail === "string") {
                  if (detail !== item.detail) throw new Error(`detail changed: ${detail}`)
                } else if (!item.detail.test(detail)) {
                  throw new Error(`detail changed: ${detail}`)
                }
                details.push(detail)
                const box = surface.getByRole("checkbox", { name: selectors.toggle })
                if ((await box.isChecked()) !== item.checked)
                  throw new Error("checkbox does not show the gateway preference")
                if (await box.isDisabled())
                  throw new Error("the preference toggle is disabled")
                const rect = await card.boundingBox()
                const surfaceBox = await surface.boundingBox()
                if (
                  !rect ||
                  !surfaceBox ||
                  rect.x < surfaceBox.x - 1 ||
                  rect.x + rect.width > surfaceBox.x + surfaceBox.width + 1
                )
                  throw new Error(
                    `card does not fit: ${JSON.stringify({ rect, surfaceBox })}`,
                  )
              }
              if (details[0] !== details[1])
                throw new Error(`surfaces disagree: ${details.join(" | ")}`)
              const text = await page.locator("body").innerText()
              if (text.includes(EARLY))
                throw new Error("a usage limit revived the early-turn notice")
              if (text.includes(REQUIRED))
                throw new Error("a pure usage limit shows the required-work sentence")
              if (item.name === "timed" && width === 1000) {
                if (options.shots) {
                  mkdirSync(options.shots, { recursive: true })
                  await desktop.locator(selectors.card).screenshot({
                    path: resolve(options.shots, `${engine}-desktop-card.png`),
                  })
                  await panel.locator(selectors.card).screenshot({
                    path: resolve(options.shots, `${engine}-panel-card.png`),
                  })
                }
                const desktopBox = desktop.getByRole("checkbox", {
                  name: selectors.toggle,
                })
                await desktopBox.uncheck()
                await page.waitForFunction(
                  () =>
                    window.__usageLimit.desktopCalls.at(-1)?.enabled === false &&
                    !document
                      .querySelector("#desktop [data-usage-limit]")
                      ?.hasAttribute("data-usage-limit-pending"),
                )
                if (await desktopBox.isChecked())
                  throw new Error("toggle did not turn off")
                const panelBox = panel.getByRole("checkbox", { name: selectors.toggle })
                await panelBox.uncheck()
                await page.waitForFunction(
                  () =>
                    window.__usageLimit.panelCalls.at(-1) === false &&
                    !document
                      .querySelector("#panel [data-usage-limit]")
                      ?.hasAttribute("data-usage-limit-pending"),
                )
                await page.evaluate(() => {
                  window.__usageLimit.fail = true
                })
                await desktopBox.click()
                await page.waitForFunction(() => {
                  const box = document.querySelector("#desktop [data-usage-limit] input")
                  return (
                    box instanceof HTMLInputElement &&
                    !box.checked &&
                    window.__usageLimit.desktopCalls.at(-1)?.enabled === true
                  )
                })
                const calls = await page.evaluate(() => window.__usageLimit.desktopCalls)
                if (calls.some((call) => call.initiator !== "person"))
                  throw new Error("the toggle was not asked by the person")
              }
              return {
                passed: true,
                detail: details[0],
                errors: opened.heldErrorCount,
              }
            } finally {
              await opened.close()
            }
          })
        }
      }
    })
  },
)
