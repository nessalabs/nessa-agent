#!/usr/bin/env node
/** Ready gateway text to production desktop DOM plus two frame opportunities (#532). */
import { resolve } from "node:path"
import { target, startPreview, repoRoot } from "./lib/server.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, messageSync } from "./lib/selectors.mjs"
const meta = {
  name: "message-sync",
  summary: "active delivery, held list/read independence and idle cost",
  defaults: { engine: "chromium,webkit", mode: "prod" },
  help: "Ten active replacements and five with a held summary/list. Asserts 600 ms delivery and idle request pacing; controlled transport, no provider startup.",
}
async function measure(page, prefix, count) {
  const samples = []
  for (let i = 0; i < count; i++) {
    // Vary publication phase reproducibly instead of publishing only just after a poll.
    await page.waitForTimeout((i * 73) % 251)
    samples.push(
      await page.evaluate(
        async ([marker, selector]) => {
          const start = window.__messageSync.publish(marker)
          return new Promise((resolve) => {
            let seen = false
            const check = () => {
              if (
                seen ||
                ![...document.querySelectorAll(selector)].some((e) =>
                  e.textContent.includes(marker),
                )
              )
                return
              seen = true
              observer.disconnect()
              clearTimeout(timer)
              requestAnimationFrame(() =>
                requestAnimationFrame(() => resolve(performance.now() - start)),
              )
            }
            const observer = new MutationObserver(check)
            observer.observe(document.body, {
              subtree: true,
              childList: true,
              characterData: true,
            })
            const timer = setTimeout(() => {
              observer.disconnect()
              resolve(null)
            }, 2_000)
            check()
          })
        },
        [`${prefix}-${i}`, css.message],
      ),
    )
  }
  return samples
}
const fixtureTarget = (options) =>
  options.mode === "prod" && !options.url
    ? startPreview(
        options,
        {},
        resolve(repoRoot, "verification/desktop/fixtures/message-sync/vite.config.ts"),
      )
    : target(options)

await main(
  meta,
  async ({ options, rep, url }) => {
    const html = await fetch(new URL(messageSync.page, url)).then((r) => r.text())
    if (!html.includes(messageSync.title))
      throw new CannotRun("message-sync fixture is not served")
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts)
        await attempt(rep, { name: "delivery", engine, layout }, async () => {
          const opened = await openPage(browser, {
            url: new URL(messageSync.page, url).href,
            layout,
          })
          const { page } = opened
          try {
            await page
              .locator(css.sessionRow, { hasText: messageSync.session })
              .first()
              .click()
            await page.locator(css.message, { hasText: "Initial answer" }).waitFor()
            const before = await page.evaluate(() => window.__messageSync.snapshot())
            const active = await measure(page, "active", 10)
            const after = await page.evaluate(() => window.__messageSync.snapshot())
            await page.evaluate(() => window.__messageSync.hold())
            await page.waitForFunction(
              () => window.__messageSync.snapshot().heldLists > 0,
              null,
              { timeout: 3_000 },
            )
            const held = await measure(page, "held", 5)
            const stalled = await page.evaluate(() => window.__messageSync.snapshot())
            await page.evaluate(() => window.__messageSync.rest())
            await page.locator(css.message, { hasText: "Finished" }).waitFor()
            // The final list changes the row read-against; let that idle refresh settle.
            await page.waitForFunction(
              (lists) => window.__messageSync.snapshot().lists >= lists + 2,
              stalled.lists,
              { timeout: 5_000 },
            )
            const idle = await page.evaluate(() => window.__messageSync.snapshot())
            await page.waitForFunction(
              (lists) => window.__messageSync.snapshot().lists >= lists + 2,
              idle.lists,
              { timeout: 4_000 },
            )
            const rested = await page.evaluate(() => window.__messageSync.snapshot())
            const failures = [...opened.errors]
            for (const [name, samples] of Object.entries({ active, held }))
              if (samples.some((ms) => ms === null || ms > 600))
                failures.push(
                  `${name}: delivery exceeded 600 ms: ${JSON.stringify(samples)}`,
                )
            if (stalled.heldReads !== 1)
              failures.push(
                `held conversation admitted ${stalled.heldReads} reads, expected one`,
              )
            const elapsed = after.now - before.now
            if (after.lists - before.lists > Math.ceil(elapsed / 1_000) + 1)
              failures.push("summary request rate increased")
            if (after.fastReads - before.fastReads > Math.ceil(elapsed / 250) + 1)
              failures.push("active request rate exceeded four per second")
            if (rested.reads !== idle.reads)
              failures.push(
                `idle transcripts read ${rested.reads - idle.reads} more times`,
              )
            return {
              failures,
              measured: {
                activeMs: active,
                heldMs: held,
                before,
                after,
                stalled,
                idle,
                rested,
              },
            }
          } finally {
            await opened.close()
          }
        })
    })
  },
  fixtureTarget,
)
