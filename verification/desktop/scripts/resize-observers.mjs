#!/usr/bin/env node
/** Pane fitting must not start a same-depth observer during resize delivery. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  {
    name: "resize-observers",
    summary: "native resize delivery stays free of skipped observations",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        await attempt(rep, { name: "fit-after-delivery", engine, layout }, async () => {
          const opened = await openPage(browser, {
            url,
            layout,
            width: 1200,
            height: 850,
            readySelector: css.workspace,
            initScripts: [
              () => {
                // Chromium does not forward this native ErrorEvent through
                // Playwright's pageerror. Observe it without cancelling it;
                // openPage keeps its independent pageerror/console checks.
                window.__nativeResizeErrors = { count: 0, messages: [] }
                window.addEventListener("error", (event) => {
                  if (typeof event.message !== "string") return
                  const errors = window.__nativeResizeErrors
                  errors.count += 1
                  if (errors.messages.length < 20)
                    errors.messages.push(event.message.slice(0, 300))
                })
              },
            ],
          })
          try {
            for (let index = 0; index < 30; index++) {
              await opened.page.setViewportSize({
                width: 360 + (index % 10) * 105,
                height: 600 + (index % 3) * 90,
              })
              await opened.page.evaluate(() => new Promise(requestAnimationFrame))
            }
            // Let the final observer broadcast and its queued fit complete.
            await opened.page.evaluate(
              () =>
                new Promise((done) =>
                  requestAnimationFrame(() => requestAnimationFrame(done)),
                ),
            )
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await opened.page.locator(css.workspace).screenshot({
                path: resolve(options.shots, `${engine}-${layout}-fitted.jpg`),
                type: "jpeg",
                quality: 70,
                scale: "css",
              })
            }
            const errors = await opened.page.evaluate(() => window.__nativeResizeErrors)
            return {
              viewportChanges: 30,
              nativeErrorCount: errors.count,
              failures: errors.messages,
            }
          } finally {
            await opened.close()
          }
        })
      }
    })
  },
)
