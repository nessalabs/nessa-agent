#!/usr/bin/env node
/** Compact composer errors, complete Copy payload, and identity-preserving recovery. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  { name: "send-failure", defaults: { engine: "chromium,webkit" } },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const surface of ["conversation", "overview"])
        for (const width of [360, 1000]) {
          const target = new URL(
            "/verification/desktop/fixtures/send-failure/index.html",
            url,
          )
          target.searchParams.set("surface", surface)
          const opened = await openPage(browser, { url: target.href, width, height: 800 })
          const page = opened.page
          try {
            const notice = page.locator(css.sendFailure)
            await notice.waitFor()
            const detail = await page.evaluate(() => window.__sendFailure)
            const text = await notice.innerText()
            if (
              !text.includes(`Error sending · ${detail.code}`) ||
              text.includes(detail.diagnostic)
            )
              throw new Error("failure is verbose or loses the actual code")
            const placement = await page.evaluate((selector) => {
              const row = document.querySelector(selector)
              const field = document.querySelector("textarea")
              return (
                row &&
                field &&
                row.getBoundingClientRect().bottom <= field.getBoundingClientRect().top
              )
            }, css.sendFailure)
            if (!placement) throw new Error("failure is not above the composer")
            await page
              .getByRole("button", { name: "Copy error details", exact: true })
              .click()
            await page.waitForFunction(() => window.__sendFailure.copied.length > 0)
            const copied = await page.evaluate(() => window.__sendFailure.copied)
            if (copied !== `${detail.code}\n${detail.diagnostic}`)
              throw new Error("Copy lost diagnostic detail")
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await page.screenshot({
                path: resolve(options.shots, `${engine}-${surface}-${width}.png`),
              })
            }
            await page.getByRole("button", { name: "Send Again", exact: true }).click()
            await page.waitForFunction(() => window.__sendFailure.attempts === 2)
            await notice.waitFor()
            if (
              surface === "conversation" &&
              (await page.locator(`${css.message}[data-role="user"]`).count()) !== 1
            )
              throw new Error("retry duplicated the failed message")
            await page.getByRole("button", { name: "Discard", exact: true }).click()
            await notice.waitFor({ state: "detached" })
            rep.add({
              name: "compact-recovery",
              engine,
              surface,
              width,
              failures: [],
              detail: "above composer; full Copy payload; retry once; discard",
            })
          } finally {
            await opened.close()
          }
        }
    })
  },
)
