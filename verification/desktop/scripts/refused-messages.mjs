#!/usr/bin/env node
/**
 * Two refused messages, the newer sent again: Send Again still sends the older
 * one. When it carries an image, Send Again puts it back and stays held until
 * the image has uploaded again.
 */
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
      for (const width of [360, 1000]) {
        const target = new URL(
          "/verification/desktop/fixtures/refused-messages/index.html?case=image",
          url,
        )
        const opened = await openPage(browser, { url: target.href, width, height: 600 })
        const page = opened.page
        const failures = []
        try {
          const notice = page.locator(css.sendFailure)
          await notice.waitFor()
          const sendAgain = page.getByRole("button", { name: "Send Again", exact: true })
          const sent = () => page.evaluate(() => window.__refused.submitted.at(-1))
          const expectHeld = async (step, held) => {
            // The button's end state, not the moment after the store changed.
            try {
              await page.waitForFunction(
                ([name, want]) =>
                  [...document.querySelectorAll("button")].find(
                    (button) => button.textContent?.trim() === name,
                  )?.disabled === want,
                ["Send Again", held],
                { timeout: 2000 },
              )
            } catch {
              failures.push(`Send Again ${held ? "live" : "held"} ${step}`)
            }
          }
          await expectHeld("before the image is put back", false)
          await sendAgain.click()
          await expectHeld("while the image waits to upload", true)
          if ((await sent()) === "first")
            failures.push("Send Again sent the message before its image uploaded")
          await page.evaluate(() => window.__upload("uploading"))
          await expectHeld("while the image uploads", true)
          await page.evaluate(() => window.__upload("failed"))
          await expectHeld("after the upload failed", true)
          // Trying the upload again, from its tile.
          await page.evaluate(() => window.__upload("not-started"))
          await expectHeld("while the image waits to upload again", true)
          await page.evaluate(() => window.__upload("uploading"))
          await page.evaluate(() => window.__upload("stored"))
          await expectHeld("after the image uploaded", false)
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.screenshot({
              path: resolve(options.shots, `${engine}-refused-image-${width}.png`),
            })
          }
          await sendAgain.click()
          await notice.waitFor({ state: "detached" })
          if ((await sent()) !== "first")
            failures.push(`Send Again sent ${JSON.stringify(await sent())}`)
        } catch (error) {
          failures.push(error instanceof Error ? error.message : String(error))
        } finally {
          await opened.close()
        }
        rep.add({
          name: "older-refusal-image-upload",
          engine,
          width,
          failures,
          detail:
            "older refusal with an image: held until the image uploads again, then sends",
        })
      }
    })
  },
)
