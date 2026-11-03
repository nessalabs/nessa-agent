#!/usr/bin/env node
/** #501: compact provider recovery, bounded action, honest launch and later replacement. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, providerSignIn as selectors } from "./lib/selectors.mjs"

const OAUTH_DIAGNOSTIC = "Internal error: OAuth session expired"
const REQUIRED_WORK = "The turn could not complete all required work."

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
                const oauth = page.getByText(OAUTH_DIAGNOSTIC, { exact: true })
                if ((await oauth.count()) !== 1)
                  throw new Error(
                    "local retry hid recovery and dropped the authentication terminal",
                  )
                if (options.shots && provider === "claude" && width === 1000) {
                  await page.screenshot({
                    path: resolve(options.shots, `${engine}-desktop-auth-terminal.png`),
                  })
                }
                await page.evaluate(() => window.__providerSignIn.failRetry())
                await page.locator(css.sendFailure).waitFor()
                if ((await oauth.count()) !== 1)
                  throw new Error(
                    "failed local retry dropped the authentication terminal",
                  )
                if (options.shots && provider === "claude" && width === 1000) {
                  await page.screenshot({
                    path: resolve(
                      options.shots,
                      `${engine}-desktop-auth-terminal-failed-retry.png`,
                    ),
                  })
                }
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
              const errorCount = opened.heldErrorCount
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
                errors: errorCount,
              })
            } finally {
              await opened.close()
            }
          }
        }
      }
      // ADR 501: Claude refused in a Boat VM offers Authenticate, which waits
      // on the browser, says when the sign-in is kept, and says which stage failed.
      for (const surface of ["desktop", "panel"]) {
        for (const width of [360, 1000]) {
          const target = new URL(
            surface === "panel" ? selectors.panelPage : selectors.page,
            url,
          )
          target.searchParams.set("provider", "claude")
          target.searchParams.set("lease", "boat")
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
            if ((await button.textContent()) !== "Authenticate")
              throw new Error("a Boat refusal does not offer Authenticate")
            const rect = await card.boundingBox()
            if (!rect || rect.height > 130 || rect.x + rect.width > width)
              throw new Error(`Boat card does not fit: ${JSON.stringify(rect)}`)
            await page.waitForFunction(
              (selector) => !document.querySelector(selector)?.disabled,
              selectors.button,
            )
            await button.click()
            await page
              .getByText("Finish signing in in your browser.", { exact: true })
              .waitFor()
            if (!(await button.isDisabled()))
              throw new Error("Authenticate in flight is not disabled")
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await page.screenshot({
                path: resolve(
                  options.shots,
                  `${engine}-${surface}-boat-${width}-waiting.png`,
                ),
              })
            }
            await page.evaluate(() => window.__providerSignIn.release())
            await page
              .getByText("Signed in for Boat workspaces. Send your message again.", {
                exact: true,
              })
              .waitFor()
            if ((await button.count()) !== 0)
              throw new Error("a kept sign-in still offers Authenticate")
            if (options.shots)
              await page.screenshot({
                path: resolve(
                  options.shots,
                  `${engine}-${surface}-boat-${width}-signed-in.png`,
                ),
              })
            const failed = {}
            for (const [stage, text] of [
              ["minted", "Could not sign in. Try again."],
              ["saved", "Could not save the sign-in. Unlock Nessa and try again."],
            ]) {
              await page.goto(target.href)
              await card.waitFor()
              await page.evaluate((at) => {
                window.__providerSignIn.boatFail = at
              }, stage)
              await page.waitForFunction(
                (selector) => !document.querySelector(selector)?.disabled,
                selectors.button,
              )
              await button.click()
              await page.evaluate(() => window.__providerSignIn.release())
              await page.getByText(text, { exact: true }).waitFor()
              await page.waitForFunction(
                (selector) => !document.querySelector(selector)?.disabled,
                selectors.button,
              )
              failed[stage] = true
            }
            const calls = await page.evaluate(() => window.__providerSignIn.calls)
            if (calls.some((value) => value !== "boat"))
              throw new Error("a Boat refusal launched this computer's sign-in")
            rep.add({
              name: "provider-sign-in-boat",
              engine,
              surface,
              width,
              passed: true,
              cardHeight: rect.height,
              failedStagesShown: Object.keys(failed),
              errors: opened.heldErrorCount,
            })
          } finally {
            await opened.close()
          }
        }
      }
      for (const [label, query, shown] of [
        ["oauth-diagnostic", { diagnostic: OAUTH_DIAGNOSTIC }, false],
        ["required-work", { diagnostic: REQUIRED_WORK, requiredWork: "failed" }, true],
      ]) {
        const target = new URL(selectors.panelPage, url)
        target.searchParams.set("provider", "claude")
        target.searchParams.set("diagnostic", query.diagnostic)
        if (query.requiredWork)
          target.searchParams.set("requiredWork", query.requiredWork)
        const opened = await openPage(browser, {
          url: target.href,
          width: 1000,
          height: 700,
          readySelector: selectors.card,
        })
        try {
          const count = await opened.page
            .getByText(query.diagnostic, { exact: true })
            .count()
          if (count !== (shown ? 1 : 0))
            throw new Error(
              `${label} diagnostic beside sign-in: expected ${shown ? 1 : 0}, saw ${count}`,
            )
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await opened.page.screenshot({
              path: resolve(options.shots, `${engine}-panel-${label}.png`),
            })
          }
          rep.add({
            name: "provider-sign-in-required-work",
            engine,
            surface: "panel",
            label,
            shown,
            count,
            passed: true,
            errors: opened.heldErrorCount,
          })
        } finally {
          await opened.close()
        }
      }
    })
  },
)
