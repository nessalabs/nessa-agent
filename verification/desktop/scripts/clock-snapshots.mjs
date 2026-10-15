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
  summary: "React clock snapshots stay cached and publish only on timer ticks",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/clock-snapshots.mjs [options]

Chromium and WebKit. The public ClockProvider/useNow hook checks that a 999 →
1000 crossing during consecutive React reads does not change the cached
snapshot, then that its published tick commits an updated render and unmount
retires the last timer. The browser reports any React console or page error.
`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    const scenarios = options.only
      ? options.only.split(",")
      : ["consecutive-reads", "published-tick-and-cleanup"]
    for (const scenario of scenarios) {
      if (!["consecutive-reads", "published-tick-and-cleanup"].includes(scenario))
        throw new Error(`unknown clock snapshot case: ${scenario}`)
      await attempt(rep, { engine, name: scenario }, async () => {
        const opened = await openPage(browser, {
          url: new URL(`${fixture}?case=${scenario}`, url).href,
          readySelector: "#clock",
        })
        const { page } = opened
        try {
          if (scenario === "consecutive-reads") {
            await page.waitForFunction(
              () => window.__clockProbe.nowValues.length >= 2,
              null,
              { timeout: 5000 },
            )
            const readCrossing = await page.evaluate(() => ({
              nowValues: window.__clockProbe.nowValues,
              renderedNow: Number(
                document.querySelector("#clock")?.getAttribute("data-now"),
              ),
            }))
            const failures = []
            if (readCrossing.nowValues[0] !== 999 || readCrossing.nowValues[1] !== 1000)
              failures.push(
                `the injected reads were ${JSON.stringify(readCrossing.nowValues.slice(0, 2))}, expected 999 then 1000`,
              )
            if (readCrossing.renderedNow !== 1000)
              failures.push(
                `the rendered clock was ${readCrossing.renderedNow}, expected 1000`,
              )
            return {
              injectedReads: readCrossing.nowValues.slice(0, 2),
              renderedNow: readCrossing.renderedNow,
              failures,
            }
          }

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
              Number(document.querySelector("#clock")?.getAttribute("data-now")) ===
                1000 &&
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
            nowValues: window.__clockProbe.nowValues,
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
          return {
            injectedNow: "999 → 1000",
            beforeTick,
            published,
            beforeUnmount,
            afterUnmount,
            failures,
          }
        } finally {
          await opened.close()
        }
      })
    }
  })
})
