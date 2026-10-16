#!/usr/bin/env node
/** Two refused messages, the newer sent again: Send Again still sends the older one. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  { name: "refused-messages", defaults: { engine: "chromium,webkit" } },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of [360, 1000]) {
        const target = new URL(
          "/verification/desktop/fixtures/refused-messages/index.html",
          url,
        )
        const opened = await openPage(browser, { url: target.href, width, height: 600 })
        const page = opened.page
        const failures = []
        try {
          const notice = page.locator(css.sendFailure)
          await notice.waitFor()
          const turns = () =>
            page.evaluate(() => document.querySelector("output")?.dataset.turns)
          // The older refusal is the one left; the newer one was taken.
          const before = (await turns())?.split(",") ?? []
          if (
            before.filter((receipt) => receipt === "failed").length !== 1 ||
            !before.some((receipt) => ["accepted", "delivered"].includes(receipt))
          )
            failures.push(`unexpected turns before Send Again: ${await turns()}`)
          const sendAgain = page.getByRole("button", { name: "Send Again", exact: true })
          if (await sendAgain.isDisabled())
            failures.push("Send Again is disabled for the older refusal")
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.screenshot({
              path: resolve(options.shots, `${engine}-refused-messages-${width}.png`),
            })
          }
          await sendAgain.click()
          await notice.waitFor({ state: "detached" })
          const submitted = await page.evaluate(() => window.__refused.submitted)
          if (submitted.at(-1) !== "first")
            failures.push(`Send Again sent ${JSON.stringify(submitted.at(-1))}`)
          if ((await turns())?.includes("failed"))
            failures.push(`a refusal is left after Send Again: ${await turns()}`)
        } catch (error) {
          failures.push(error instanceof Error ? error.message : String(error))
        } finally {
          await opened.close()
        }
        rep.add({
          name: "older-refusal-send-again",
          engine,
          width,
          failures,
          detail:
            "older refusal offered after the newer was sent again; sends its own text",
        })
      }
    })
  },
)
