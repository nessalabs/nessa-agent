#!/usr/bin/env node
/** A gateway frame's text to production desktop DOM plus two frame opportunities (#532, #702). */
import { resolve } from "node:path"
import { mkdirSync } from "node:fs"
import { target, startPreview, repoRoot } from "./lib/server.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import {
  attempt,
  CannotRun,
  chosen,
  recordIfLeftOut,
  devServerOnlySteps,
} from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { appFrame } from "./lib/apps.mjs"
import { css, messageSync, selectorFor } from "./lib/selectors.mjs"
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
  summary: "active delivery, held list/conversation independence and idle cost",
  defaults: { engine: "chromium,webkit", mode: "prod" },
  options: {
    only: { type: "string" },
    runs: { type: "string", default: "3" },
    throttle: { type: "string", default: "4" },
  },
  help: "--only retained-app|delivery. retained-app requires the dev sandbox and preserves the exact app frame across text changes. Three fresh-page runs per engine/layout; calibrated 4x Chromium, unthrottled WebKit. Ten active replacements and five while the list's and another conversation's frames are held. Asserts 600 ms delivery, that delivery opens no subscription, and that a window at rest asks nothing; controlled transport, no provider startup.",
}
const stats = (samples) => ({
  samples: samples.length,
  medianMs: samples.every(Number.isFinite) ? median(samples) : null,
  maxMs: samples.every(Number.isFinite) ? Math.max(...samples) : null,
})
async function measure(page, prefix, count) {
  const samples = []
  for (let i = 0; i < count; i++) {
    // Vary publication phase reproducibly.
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
  async ({ options, rep, url, mode }) => {
    const html = await fetch(new URL(messageSync.page, url)).then((r) => r.text())
    if (!html.includes(messageSync.title))
      throw new CannotRun("message-sync fixture is not served")
    const selected = options.only
      ? chosen(options.only, ["retained-app", "delivery"], options.list)
      : ["retained-app", "delivery"]
    const runs = Number(options.runs)
    const rate = Number(options.throttle)
    if (!Number.isInteger(runs) || runs < 1 || !Number.isFinite(rate) || rate < 1)
      throw new CannotRun("runs must be a positive integer and throttle at least one")
    const summaries = []
    await withEngines(options, rep, async (engine, browser) => {
      if (engine === "chromium" && selected.includes("delivery")) {
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
      for (const layout of options.layouts) {
        if (!selected.includes("retained-app")) continue
        if (
          recordIfLeftOut(rep, mode, "retained-app", devServerOnlySteps["message-sync"], {
            engine,
            layout,
          })
        )
          continue
        await attempt(rep, { name: "retained-app", engine, layout }, async () => {
          const opened = await openPage(browser, {
            url: new URL(messageSync.page, url).href,
            layout,
          })
          try {
            const { page } = opened
            await page
              .locator(css.sessionRow, { hasText: messageSync.session })
              .first()
              .click()
            await page.evaluate(() => window.__messageSync.app("before"))
            const held = await appFrame(page, "inline")
            await held.app.waitForSelector(selectorFor.fixtureState("live"))
            await held.app.click(selectorFor.fixtureControl("call-allowed"))
            await held.app.waitForFunction(
              (selector) =>
                document.querySelector(selector)?.textContent.startsWith("ok:"),
              selectorFor.fixtureOutput("call"),
            )
            await held.app.evaluate(() => {
              window.__retainedWidget = "kept"
            })
            const failures = []
            for (const prefix of ["added", "removed"]) {
              await page.evaluate((prefix) => window.__messageSync.app(prefix), prefix)
              await page.waitForFunction(
                ([message, prefix]) => {
                  const text = [...document.querySelectorAll(message)]
                    .map((element) => element.textContent)
                    .join(" ")
                  return prefix === "added"
                    ? text.includes("More text")
                    : !text.includes("Earlier text") && !text.includes("More text")
                },
                [css.message, prefix],
              )
              const now = await appFrame(page, "inline")
              if (
                now.app !== held.app ||
                now.proxy !== held.proxy ||
                held.app.isDetached()
              )
                failures.push(`${prefix} text replaced the existing app frame`)
              if ((await now.app.evaluate(() => window.__retainedWidget)) !== "kept")
                failures.push(`${prefix} text lost the app's document state`)
            }
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await page
                .locator(css.surface)
                .first()
                .screenshot({
                  path: resolve(options.shots, `${engine}-${layout}-retained-app.jpg`),
                  type: "jpeg",
                  quality: 70,
                  scale: "css",
                })
            }
            return { failures }
          } finally {
            await opened.close()
          }
        })
      }
      if (!selected.includes("delivery")) return
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
              await page.evaluate(() => window.__messageSync.start())
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
              const failures = []
              for (const [name, samples] of Object.entries({ active, held }))
                if (samples.some((ms) => ms === null || ms > 600))
                  failures.push(
                    `${name}: delivery exceeded 600 ms: ${JSON.stringify(samples)}`,
                  )
              // Keep the delivery failure and samples before secondary idle waits.
              if (failures.length)
                return {
                  failures,
                  measured: {
                    activeMs: active,
                    heldMs: held,
                    before,
                    after,
                    stalled,
                    timeoutDiagnostics: await page.evaluate(
                      () => window.__messageSyncTimeouts ?? [],
                    ),
                  },
                }
              await page.evaluate(() => window.__messageSync.rest())
              await page.locator(css.message, { hasText: "Finished" }).waitFor()
              const idle = await page.evaluate(() => window.__messageSync.snapshot())
              // At rest nothing changes, so nothing is asked: no timer reads again.
              await page.waitForTimeout(3_000)
              const rested = await page.evaluate(() => window.__messageSync.snapshot())
              if (stalled.subscribes !== before.subscribes)
                failures.push(
                  `held frames opened ${stalled.subscribes - before.subscribes} subscriptions`,
                )
              if (after.subscribes !== before.subscribes)
                failures.push(
                  `active delivery opened ${after.subscribes - before.subscribes} subscriptions`,
                )
              if (
                rested.subscribes !== idle.subscribes ||
                rested.observes !== idle.observes
              )
                failures.push(
                  `a window at rest asked again: ${JSON.stringify({ idle, rested })}`,
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
    if (selected.includes("delivery"))
      rep.add({ name: "delivery-statistics", summaries, aggregates, failures: [] })
  },
  fixtureTarget,
)
