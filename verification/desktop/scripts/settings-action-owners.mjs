#!/usr/bin/env node
/** #1087: native save and notification prompt admission/lifetime. */
import assert from "node:assert/strict"
import { chromium } from "playwright"
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { launch, openPage } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { settingsActionOwners as ui } from "./lib/selectors.mjs"

await main(
  {
    name: "settings-action-owners",
    summary:
      "one pending settings action; retired saves/prompts cannot publish into replacement owners",
    defaults: { engine: "chromium,webkit" },
    options: { "chromium-executable": { type: "string" } },
    help: "Production settings components over controlled sources. No OS prompt or harness execution. Dev fixture only. --chromium-executable accepts an installed Chromium.",
  },
  async ({ options, rep, url, mode }) => {
    if (mode === "prod")
      throw new CannotRun("settings-action-owners needs its dev fixture")
    for (const engine of options.engine.split(",")) {
      const browser =
        engine === "chromium" && options["chromium-executable"]
          ? await chromium.launch({
              executablePath: options["chromium-executable"],
              headless: !options.headed,
            })
          : await launch(engine, options)
      let opened
      try {
        opened = await openPage(browser, {
          url: new URL(ui.page, url).href,
          width: 1200,
          height: 1000,
          readySelector: ui.ready,
        })
        const { page } = opened
        const fresh = async () => {
          await page.goto(new URL(ui.page, url).href)
          await page.locator(ui.claude).waitFor()
          await page.locator(ui.notices).locator(ui.allow).waitFor()
        }
        const counts = async (selector) =>
          JSON.parse(await page.locator(selector).textContent())
        const button = (name) => page.getByRole("button", { name, exact: true })
        await attempt(
          rep,
          { engine, name: "native-admission-through-reread" },
          async () => {
            await fresh()
            await page.locator(ui.claude).evaluate((element) => {
              element.click()
              element.click()
            })
            assert.equal((await counts(ui.nativeCounts)).writesA, 1)
            assert(await page.locator(ui.codex).isDisabled())
            await button(ui.acknowledgeA).click()
            await page.waitForFunction(
              (selector) =>
                JSON.parse(document.querySelector(selector).textContent).readsA === 2,
              ui.nativeCounts,
            )
            await page.evaluate(() => {
              window.dispatchEvent(new Event("focus"))
              document.dispatchEvent(new Event("visibilitychange"))
            })
            assert.equal((await counts(ui.nativeCounts)).readsA, 2)
            assert(await page.locator(ui.claude).isDisabled())
            await button(ui.rereadA).click()
            await page.waitForFunction(
              (selector) => !document.querySelector(selector).disabled,
              ui.claude,
            )
            assert.equal(
              await page.locator(ui.claude).getAttribute("aria-checked"),
              "true",
            )
            return {
              writesBeforeSettlement: 1,
              readsDuringReconciliation: 2,
              recoveryEnabled: true,
            }
          },
        )
        await attempt(rep, { engine, name: "native-source-replacement" }, async () => {
          await fresh()
          await page.locator(ui.claude).click()
          await button(ui.replaceNative).click()
          await page.waitForFunction(
            (selector) =>
              JSON.parse(document.querySelector(selector).textContent).readsB === 1,
            ui.nativeCounts,
          )
          await page.locator(ui.codex).click()
          await button(ui.acknowledgeA).click()
          assert.equal((await counts(ui.nativeCounts)).readsA, 1)
          assert.equal((await counts(ui.nativeCounts)).writesB, 1)
          assert.equal(
            await page.locator(ui.claude).getAttribute("aria-checked"),
            "false",
          )
          assert.equal(await page.locator(ui.codex).getAttribute("aria-checked"), "true")
          assert(await page.locator(ui.codex).isDisabled())
          await button(ui.refuseB).click()
          await page.waitForFunction(
            (selector) => !document.querySelector(selector).disabled,
            ui.codex,
          )
          return {
            retiredSourceReads: 1,
            replacementWrites: 1,
            replacementRetained: true,
          }
        })
        await attempt(rep, { engine, name: "notification-api-admission" }, async () => {
          await fresh()
          const allow = page.locator(ui.notices).locator(ui.allow)
          await allow.click()
          await allow.evaluate((element) => element.click())
          assert.equal((await counts(ui.noticeCounts)).asksA, 1)
          assert(await allow.isDisabled())
          await button(ui.grantA).click()
          await page.waitForFunction(
            (selector) =>
              JSON.parse(document.querySelector(selector).textContent).replayA === 1,
            ui.noticeCounts,
          )
          assert.equal(await allow.count(), 0)
          return {
            permissionApiCalls: 1,
            waitingReviewReplays: 1,
            realOsPrompts: "not exercised",
          }
        })
        await attempt(
          rep,
          { engine, name: "notification-source-replacement" },
          async () => {
            await fresh()
            await page.locator(ui.notices).locator(ui.allow).click()
            await button(ui.replaceNotice).click()
            const allow = page.locator(ui.notices).locator(ui.allow)
            await allow.click()
            await button(ui.grantA).click()
            assert.equal((await counts(ui.noticeCounts)).replayB, 0)
            assert(await allow.isDisabled())
            await button(ui.grantB).click()
            await page.waitForFunction(
              (selector) =>
                JSON.parse(document.querySelector(selector).textContent).replayB === 1,
              ui.noticeCounts,
            )
            return {
              retiredReplayCalls: 0,
              currentReplayCalls: 1,
              productionRebinding: "unproven",
            }
          },
        )
        if (options.shots) {
          mkdirSync(options.shots, { recursive: true })
          await page.screenshot({
            path: join(options.shots, `${engine}-settings-action-owners.png`),
          })
        }
        rep.add({ engine, name: "console" })
      } finally {
        await opened?.close()
        await browser.close()
      }
    }
  },
)
