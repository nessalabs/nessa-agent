#!/usr/bin/env node
/** Ready gateway text to production desktop DOM plus two frame opportunities (#532). */
import { resolve } from "node:path"
import { mkdirSync } from "node:fs"
import { target, startPreview, repoRoot } from "./lib/server.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, messageSync } from "./lib/selectors.mjs"
import {
  calibrate,
  calibrationFrame,
  interactionBudget,
  measure as measureFrames,
  median,
  observers,
  throttle,
} from "./lib/perf.mjs"
const meta = {
  name: "message-sync",
  summary: "active delivery, held list/read independence and idle cost",
  defaults: { engine: "chromium,webkit", mode: "prod" },
  options: {
    runs: { type: "string", default: "3" },
    throttle: { type: "string", default: "4" },
  },
  help: "Three fresh-page runs per engine/layout; calibrated 4x Chromium, unthrottled WebKit. Ten active replacements and five with a held summary/list. Asserts 600 ms delivery and idle request pacing; controlled transport, no provider startup.",
}
const stats = (samples) => ({
  samples: samples.length,
  medianMs: samples.every(Number.isFinite) ? median(samples) : null,
  maxMs: samples.every(Number.isFinite) ? Math.max(...samples) : null,
})
async function measure(page, prefix, count) {
  const samples = []
  for (let i = 0; i < count; i++) {
    // Vary publication phase reproducibly instead of publishing only just after a poll.
    await page.waitForTimeout((i * 73) % 251)
    samples.push(
      await page.evaluate(
        async ([marker, selector]) => {
          const before = window.__messageSync.snapshot()
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
              requestAnimationFrame(() =>
                requestAnimationFrame(() => {
                  clearTimeout(timer)
                  resolve(performance.now() - start)
                }),
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
              ;(window.__messageSyncTimeouts ??= []).push({
                marker,
                seen,
                visibility: document.visibilityState,
                before,
                after: window.__messageSync.snapshot(),
                rendered: [...document.querySelectorAll(selector)].map(
                  (e) => e.textContent,
                ),
                elapsedMs: performance.now() - start,
              })
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
    const runs = Number(options.runs)
    const rate = Number(options.throttle)
    if (!Number.isInteger(runs) || runs < 1 || !Number.isFinite(rate) || rate < 1)
      throw new CannotRun("runs must be a positive integer and throttle at least one")
    const summaries = []
    await withEngines(options, rep, async (engine, browser) => {
      if (engine === "chromium") {
        const calibration = await attempt(
          rep,
          { name: "calibration", engine },
          async () => {
            const opened = await openPage(browser, {
              url: new URL(messageSync.page, url).href,
              initScripts: [observers],
            })
            try {
              const loop = await calibrate(opened.context, opened.page, rate)
              const frame = await calibrationFrame(opened.page)
              return {
                calibration: loop,
                calibrationFrame: frame,
                failures: [
                  ...opened.errors,
                  ...(!loop.ok ? ["CPU throttle calibration failed"] : []),
                  ...(!frame.ok ? ["Known-cost frame calibration failed"] : []),
                ],
              }
            } finally {
              await opened.close()
            }
          },
        )
        if (!calibration.ok) return
      }
      for (const layout of options.layouts)
        for (let run = 1; run <= runs; run++)
          await attempt(rep, { name: "delivery", engine, layout, run }, async () => {
            const opened = await openPage(browser, {
              url: new URL(messageSync.page, url).href,
              layout,
              initScripts: [observers],
            })
            const { page } = opened
            const shot = async (state) => {
              if (!options.shots || run !== 1) return
              mkdirSync(options.shots, { recursive: true })
              await page
                .locator(css.surface)
                .first()
                .screenshot({
                  path: resolve(options.shots, `${engine}-${layout}-${state}.jpg`),
                  type: "jpeg",
                  quality: 75,
                  scale: "css",
                })
            }
            try {
              await page
                .locator(css.sessionRow, { hasText: messageSync.session })
                .first()
                .click()
              await page.locator(css.message, { hasText: "Initial answer" }).waitFor()
              if (engine === "chromium") await throttle(opened.context, page, rate)
              await page.waitForTimeout(400)
              const before = await page.evaluate(() => window.__messageSync.snapshot())
              let active
              const activeFrames = await measureFrames(
                page,
                async () => {
                  active = await measure(page, "active", 10)
                },
                100,
              )
              const after = await page.evaluate(() => window.__messageSync.snapshot())
              await shot("active-transcript")
              await page.evaluate(() => window.__messageSync.hold())
              await page.waitForFunction(
                () => window.__messageSync.snapshot().heldLists > 0,
                null,
                { timeout: 3_000 },
              )
              let held
              const heldFrames = await measureFrames(
                page,
                async () => {
                  held = await measure(page, "held", 5)
                },
                100,
              )
              const stalled = await page.evaluate(() => window.__messageSync.snapshot())
              await shot("held-list-transcript")
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
              const frames = interactionBudget([activeFrames, heldFrames])
              if (engine === "chromium" && frames.exceeded)
                failures.push(`message update frame ${frames.maxFrame} ms exceeds 50 ms`)
              summaries.push({
                engine,
                layout,
                run,
                throttle: engine === "chromium" ? rate : 1,
                active: stats(active),
                held: stats(held),
                frameMaxMs: frames.maxFrame,
                activeMs: active,
                heldMs: held,
                over50: frames.over50,
              })
              await shot("idle-transcript")
              return {
                failures,
                measured: {
                  timeoutDiagnostics: await page.evaluate(
                    () => window.__messageSyncTimeouts ?? [],
                  ),
                  activeMs: active,
                  activeSummary: stats(active),
                  heldSummary: stats(held),
                  throttle: engine === "chromium" ? rate : 1,
                  activeFrames,
                  heldFrames,
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
    const aggregates = []
    for (const engine of options.engines)
      for (const layout of options.layouts) {
        const group = summaries.filter(
          (row) => row.engine === engine && row.layout === layout,
        )
        if (!group.length) continue
        aggregates.push({
          engine,
          layout,
          runs: group.length,
          active: stats(group.flatMap((row) => row.activeMs)),
          held: stats(group.flatMap((row) => row.heldMs)),
          frameMaxMs: Math.max(...group.map((row) => row.frameMaxMs)),
          frameRunMedianMs: median(group.map((row) => row.frameMaxMs)),
          over50: group.reduce((sum, row) => sum + row.over50, 0),
        })
      }
    rep.add({ name: "delivery-statistics", summaries, aggregates, failures: [] })
  },
  fixtureTarget,
)
