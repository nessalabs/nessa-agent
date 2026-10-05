#!/usr/bin/env node
/** #501: compact provider recovery, bounded action, honest launch and later replacement. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { providerSignIn as selectors } from "./lib/selectors.mjs"

await main(
  { name: "provider-sign-in", defaults: { engine: "chromium,webkit" } },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const surface of ["desktop", "panel"]) {
        for (const provider of ["claude", "codex"]) {
          for (const width of [360, 1000]) {
            const target = new URL(
              surface === "panel" ? selectors.panelPage : selectors.page,
              url,
            )
            target.searchParams.set("provider", provider)
            const opened = await openPage(browser, {
              url: target.href,
              width,
              height: 700,
              readySelector: selectors.card,
            })
            const { page } = opened
            try {
              const card = page.locator(selectors.card)
              const button = page.locator(selectors.button)
              const rect = await card.boundingBox()
              const buttonRect = await button.boundingBox()
              if (
                !rect ||
                !buttonRect ||
                rect.height > 130 ||
                rect.x < 0 ||
                rect.x + rect.width > width ||
                buttonRect.x + buttonRect.width > rect.x + rect.width
              )
                throw new Error(
                  `recovery card does not fit: ${JSON.stringify({ rect, buttonRect })}`,
                )
              if ((await card.locator("h3").textContent()) !== "Your login expired")
                throw new Error("heading changed")
              if (options.shots) {
                mkdirSync(options.shots, { recursive: true })
                await page.screenshot({
                  path: resolve(
                    options.shots,
                    `${engine}-${surface}-${provider}-${width}-normal.png`,
                  ),
                })
              }
              if (
                await page
                  .getByText("Internal error: OAuth session expired", { exact: true })
                  .count()
              )
                throw new Error("authentication diagnostic is duplicated below the card")
              if (
                (await page
                  .getByText("Nessa declined a tool review.", { exact: true })
                  .count()) !== 1
              )
                throw new Error("unrelated local notice was suppressed")
              await button.waitFor({ state: "visible" })
              await page.waitForFunction(
                (selector) => !document.querySelector(selector)?.disabled,
                selectors.button,
              )
              await button.focus()
              await page.keyboard.press("Enter")
              if (!(await button.isDisabled()))
                throw new Error("login in flight is not disabled")
              await page.evaluate(() => {
                window.__providerSignIn.release()
              })
              await page.waitForFunction(
                () => !document.querySelector(".provider-sign-in button")?.disabled,
              )
              if ((await card.count()) !== 1)
                throw new Error("launch falsely dismissed refusal")
              await page.evaluate(() => {
                window.__providerSignIn.fail = true
              })
              await button.click()
              await page.evaluate(() => {
                window.__providerSignIn.release()
              })
              await page.locator(selectors.failure).waitFor()
              const calls = await page.evaluate(() => window.__providerSignIn.calls)
              if (calls.length !== 2 || calls.some((value) => value !== provider))
                throw new Error("incorrect provider login action")
              if (options.shots) {
                mkdirSync(options.shots, { recursive: true })
                await page.screenshot({
                  path: resolve(
                    options.shots,
                    `${engine}-${surface}-${provider}-${width}.png`,
                  ),
                })
              }
              if (surface === "desktop") {
                await page.evaluate(() => window.__providerSignIn.sendRetry())
                await card.waitFor({ state: "detached" })
                await page.getByText("Try again", { exact: true }).waitFor()
                await page.evaluate(() => window.__providerSignIn.failRetry())
                await page.locator(".workspace-message-failed").waitFor()
                if ((await card.count()) !== 0)
                  throw new Error("failed newer outbox turn retained old recovery")
                for (const phase of ["acceptRetry", "runRetry", "finishRetry"]) {
                  await page.evaluate(
                    (action) => window.__providerSignIn[action](),
                    phase,
                  )
                  await page.waitForFunction(
                    () => window.__providerSignIn.outboxCount() === 0,
                  )
                  if ((await card.count()) !== 0)
                    throw new Error(`${phase} revived old authentication recovery`)
                  if ((await page.getByText("Try again", { exact: true }).count()) !== 1)
                    throw new Error(`${phase} duplicated the accepted retry input`)
                }
              }
              await page.evaluate(() => window.__providerSignIn.olderFailure())
              await card.waitFor({ state: "visible" })
              await page.getByText("Older local failure", { exact: true }).waitFor()
              await page.evaluate(() => window.__providerSignIn.repeatRefusal())
              if ((await card.count()) !== 1)
                throw new Error("repeated view hid later refusal")
              await page.evaluate(() => window.__providerSignIn.newerFailure())
              await card.waitFor({ state: "detached" })
              await page.evaluate(() => window.__providerSignIn.repeatRefusal())
              if ((await card.count()) !== 0)
                throw new Error("repeated view revived older refusal")
              await page.evaluate(() => window.__providerSignIn.newRefusal())
              await card.waitFor({ state: "visible" })
              await page.evaluate(() => {
                window.__providerSignIn.recover()
              })
              await card.waitFor({ state: "detached" })
              if (surface === "panel") {
                const historical = page.locator("[data-slot=transcript-divider]").first()
                await historical.waitFor()
                if (!(await historical.textContent()).includes("failed"))
                  throw new Error("retired recovery erased the historical failed status")
              }
              target.searchParams.set("login", "unsupported")
              await page.goto(target.href)
              await page.locator(selectors.card).waitFor()
              const unsupported = page.locator(selectors.button)
              if (!(await unsupported.isDisabled()))
                throw new Error("unsupported host offers an enabled login action")
              await unsupported.evaluate((element) => element.click())
              const unsupportedCalls = await page.evaluate(
                () => window.__providerSignIn.calls,
              )
              if (unsupportedCalls.length !== 0)
                throw new Error("unsupported host attempted provider login")
              if (opened.errors.length) throw new Error(opened.errors.join("; "))
              rep.add({
                name: "provider-sign-in",
                engine,
                provider,
                surface,
                width,
                passed: true,
                cardHeight: rect.height,
                cardWidth: rect.width,
                buttonHeight: buttonRect.height,
                loginCalls: calls.length,
                unsupportedLoginCalls: unsupportedCalls.length,
                errors: opened.errors.length,
              })
            } finally {
              await opened.close()
            }
          }
        }
      }
    })
  },
)
