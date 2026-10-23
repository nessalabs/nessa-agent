#!/usr/bin/env node
/** #1087: stale offer publication and cross-tab pending-send ownership, no gateway required. */
import assert from "node:assert/strict"
import { chromium } from "playwright"
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { launch, openPage } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { userFlowOrdering as ui } from "./lib/selectors.mjs"

await main(
  {
    name: "user-flow-ordering",
    summary:
      "obsolete offers, cross-tab sends and focus refreshes retain their pending action owners",
    defaults: { engine: "chromium,webkit" },
    options: { "chromium-executable": { type: "string" } },
    help: "Drives production AgentDownloads/useComposer through injected ports in both engines. Dev fixture only. --chromium-executable uses an already installed Chromium.",
  },
  async ({ options, rep, url, mode }) => {
    if (mode === "prod")
      throw new CannotRun("user-flow-ordering requires its dev fixture")
    for (const engine of options.engine.split(",")) {
      let browser
      try {
        browser =
          engine === "chromium" && options["chromium-executable"]
            ? await chromium.launch({
                executablePath: options["chromium-executable"],
                headless: !options.headed,
              })
            : await launch(engine, options)
        const opened = await openPage(browser, {
          url: new URL(ui.page, url).href,
          width: 1100,
          height: 850,
          readySelector: ui.ready,
        })
        const { page } = opened
        try {
          for (const reason of ["busy", "not-confirmed"]) {
            await attempt(
              rep,
              { engine, name: `uncertain-install-${reason}` },
              async () => {
                await page.goto(new URL(`${ui.page}?reason=${reason}`, url).href)
                await page.locator(ui.download).waitFor()
                await page.locator(ui.download).click()
                await page
                  .getByRole("button", { name: "Refresh during install", exact: true })
                  .click()
                await page
                  .getByRole("button", { name: "Finish uncertain install", exact: true })
                  .click()
                assert(
                  (await page.locator(ui.download).count()) === 0,
                  "uncertain outcome must hide Download",
                )
                await page
                  .getByRole("button", { name: "Release earlier offers", exact: true })
                  .click()
                assert(
                  (await page.locator(ui.download).count()) === 0,
                  "older offers restored Download after uncertainty",
                )
                assert(
                  (await page.locator(ui.installRequests).textContent()) === "1",
                  "expected one frontend install request",
                )
                await page
                  .getByRole("button", { name: "Check downloads", exact: true })
                  .click()
                await page.locator(ui.download).waitFor()
                return {
                  installDispatches: 1,
                  staleDownloadButtons: 0,
                  freshDownloadButtons: 1,
                }
              },
            )
          }
          for (const newerSend of [false, true]) {
            await attempt(
              rep,
              { engine, name: newerSend ? "old-refusal-new-send" : "cross-tab-clear" },
              async () => {
                await page.goto(new URL(ui.page, url).href)
                await page
                  .getByRole("button", { name: "Send draft", exact: true })
                  .click()
                await page
                  .getByRole("button", { name: "Switch to B", exact: true })
                  .click()
                await page
                  .getByRole("button", { name: "Expand editor", exact: true })
                  .click()
                assert(
                  (await page.locator(ui.expanded).textContent()) === "true",
                  "tab B did not expand",
                )
                if (newerSend) {
                  await page
                    .getByRole("button", { name: "Send draft", exact: true })
                    .click()
                  await page
                    .getByRole("button", { name: "Refuse first send", exact: true })
                    .click()
                }
                await page
                  .getByRole("button", { name: "Clear draft", exact: true })
                  .click()
                await page.waitForFunction(
                  ({ selector, expected }) =>
                    document.querySelector(selector)?.textContent === expected,
                  { selector: ui.expanded, expected: String(!newerSend) },
                  { timeout: 5000 },
                )
                const expanded = await page.locator(ui.expanded).textContent()
                assert(
                  expanded === String(!newerSend),
                  `tab B expanded=${expanded}; expected ${!newerSend}`,
                )
                return { tab: "B", expanded: !newerSend }
              },
            )
          }
          await attempt(rep, { engine, name: "permission-revoke-focus" }, async () => {
            await page.goto(new URL(ui.page, url).href)
            const section = page.locator(ui.permissionSection)
            const button = section.getByRole("button", { name: "Revoke", exact: true })
            await button.waitFor()
            await button.click()
            assert(
              await button.isDisabled(),
              "pending revoke did not disable its control",
            )
            await page.evaluate(() => window.dispatchEvent(new Event("focus")))
            assert(await button.isDisabled(), "focus refresh released pending revoke")
            await button.evaluate((element) => element.click())
            assert(
              (await page.locator(ui.revokeRequests).textContent()) === "1",
              "expected one pending revoke dispatch",
            )
            await section
              .getByRole("button", { name: "Finish revoke", exact: true })
              .click()
            await section
              .getByText("Could not revoke that rule.", { exact: true })
              .waitFor()
            assert(!(await button.isDisabled()), "settled revoke did not permit recovery")
            return {
              revokesBeforeOutcome: 1,
              pendingDisabled: true,
              recoveryEnabled: true,
            }
          })
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.screenshot({
              path: join(options.shots, `${engine}-user-flow-ordering.png`),
            })
          }
          rep.add({ engine, name: "console" })
        } finally {
          await opened.close()
        }
      } catch (error) {
        if (error instanceof CannotRun) throw error
        throw new CannotRun(`${engine} verification unavailable: ${error.message}`)
      } finally {
        await browser?.close()
      }
    }
  },
)
