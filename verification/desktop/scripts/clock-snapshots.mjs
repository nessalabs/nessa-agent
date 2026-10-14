#!/usr/bin/env node
/**
 * A real React hook crosses an injected clock bucket between React reads.
 * The published snapshot stays stable, the timer publishes, and unmount retires it.
 */
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"

const fixture = "/verification/desktop/fixtures/clock-snapshots/"
const meta = {
  name: "clock-snapshots",
  summary: "React reads a published clock snapshot across an injected bucket change",
  defaults: { engine: "chromium,webkit" },
  help: `
Usage: node verification/desktop/scripts/clock-snapshots.mjs [options]

Chromium and WebKit. A public ClockProvider/useNow hook reads injected time
across 999 → 1000, publishes a timer tick, then retires its timer on unmount.
The browser reports any React console or page error.
`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    await attempt(rep, { engine, name: "published-tick-and-cleanup" }, async () => {
      const opened = await openPage(browser, {
        url: new URL(fixture, url).href,
        readySelector: "#clock",
      })
      const { page } = opened
      try {
        const beforeTick = await page.locator("#clock").evaluate((element) => ({
          now: Number(element.getAttribute("data-now")),
          renders: Number(element.getAttribute("data-renders")),
        }))
        await page.evaluate(() => {
          window.__clockProbe.advance()
          window.__clockProbe.publishTick()
        })
        await page.waitForFunction(
          (rendersBeforeTick) =>
            window.__clockProbe?.timerTicks === 1 &&
            Number(document.querySelector("#clock")?.getAttribute("data-now")) === 1000 &&
            Number(document.querySelector("#clock")?.getAttribute("data-renders")) >
              rendersBeforeTick,
          beforeTick.renders,
          { timeout: 5000 },
        )
        const published = await page.locator("#clock").evaluate((element) => ({
          now: Number(element.getAttribute("data-now")),
          renders: Number(element.getAttribute("data-renders")),
        }))
        const beforeUnmount = await page.evaluate(() => ({ ...window.__clockProbe }))
        await page.evaluate(() => window.__clockProbe.unmount())
        await page.waitForFunction(
          () =>
            window.__clockProbe.timerStarts > 0 &&
            window.__clockProbe.timerStops === window.__clockProbe.timerStarts,
          null,
          { timeout: 3000 },
        )
        const afterUnmount = await page.evaluate(() => ({
          nowReads: window.__clockProbe.nowReads,
          renders: window.__clockProbe.renders,
          timerStarts: window.__clockProbe.timerStarts,
          timerTicks: window.__clockProbe.timerTicks,
          timerStops: window.__clockProbe.timerStops,
          intervals: window.__clockProbe.intervals,
        }))
        const failures = []
        if (beforeTick.now !== 999)
          failures.push(
            `the rendered clock before the tick was ${beforeTick.now}, expected 999`,
          )
        if (published.now !== 1000)
          failures.push(`the rendered clock was ${published.now}, expected 1000`)
        if (published.renders <= beforeTick.renders)
          failures.push(
            `React rendered ${published.renders} times after starting at ${beforeTick.renders}; expected the published tick to render`,
          )
        if (beforeUnmount.timerTicks !== 1)
          failures.push(
            `the interval published ${beforeUnmount.timerTicks} ticks before unmount, expected 1`,
          )
        if (
          !beforeUnmount.intervals.length ||
          beforeUnmount.intervals.some((delay) => delay !== 1000)
        )
          failures.push(
            `the clock requested unexpected intervals: ${JSON.stringify(beforeUnmount.intervals)}`,
          )
        if (
          afterUnmount.timerStarts === 0 ||
          afterUnmount.timerStops !== afterUnmount.timerStarts
        )
          failures.push(`timer cleanup was incomplete: ${JSON.stringify(afterUnmount)}`)
        if (opened.errors.length)
          failures.push(`browser reported errors: ${opened.errors.join("; ")}`)
        return {
          injectedNow: "999 → 1000",
          beforeTick,
          published,
          beforeUnmount,
          afterUnmount,
          browserErrors: opened.errors,
          failures,
        }
      } finally {
        await opened.close()
      }
    })
  })
})
