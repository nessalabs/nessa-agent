#!/usr/bin/env node
/** Retired local tab callbacks do not publish into a restored tab. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { readLifetimeCounters } from "./lib/lifetime-snapshot.mjs"
import { css, localLifetimeRoles } from "./lib/selectors.mjs"

await main(
  {
    name: "local-tab-lifetimes",
    summary: "fresh tab restoration retains follow and completion ownership",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url, mode }) => {
    if (mode === "prod" && !options.given("url"))
      throw new CannotRun(
        "the local lifetime fixture requires an explicit fixture-serving URL",
      )
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of options.quick ? [600] : [360, 600, 1440]) {
        for (const same of [false, true]) {
          const opened = await openPage(browser, {
            url: new URL(
              `/verification/conversation/lifetime-fixture.html${same ? "?same" : ""}`,
              url,
            ).href,
            readySelector: css.localLifetimes,
            width,
            mac: false,
          })
          const { page } = opened
          try {
            await attempt(
              rep,
              { engine, width, name: `restore:${same ? "same" : "different"}` },
              async () => {
                const original = await page
                  .locator(css.localLifetimes)
                  .getAttribute("data-local-id")
                await page.getByRole(...localLifetimeRoles.start).click()
                await page.getByRole(...localLifetimeRoles.restore).click()
                await page.waitForFunction(
                  (selector) =>
                    document.querySelector(selector)?.dataset.phase === "replaced",
                  css.localLifetimes,
                )
                const replacement = await page
                  .locator(css.localLifetimes)
                  .getAttribute("data-local-id")
                await page.getByRole(...localLifetimeRoles.finish).click()
                await page.waitForFunction(
                  (selector) =>
                    document.querySelector(selector)?.dataset.phase === "finished",
                  css.localLifetimes,
                )
                const state = await page.locator(css.localLifetimes).evaluate((node) => ({
                  errors: node.dataset.error,
                  statuses: node.querySelectorAll('[role="status"]').length,
                  local: node.dataset.localId,
                  overflow: node.scrollWidth > node.clientWidth + 1,
                }))
                const counters = await readLifetimeCounters((read) => page.evaluate(read))
                const seen = { ...state, ...counters }
                if (options.shots && engine === "chromium" && width === 600 && !same) {
                  mkdirSync(options.shots, { recursive: true })
                  await page.locator(css.localLifetimes).screenshot({
                    path: resolve(options.shots, "restored-tab.png"),
                  })
                }
                const A = "00000000-0000-4000-8000-000000000001"
                const B = "00000000-0000-4000-8000-000000000002"
                return {
                  original,
                  replacement,
                  ...seen,
                  failures: [
                    ...(replacement === original
                      ? ["restoration reused local lifetime"]
                      : []),
                    ...(seen.local !== replacement
                      ? ["old completion changed replacement identity"]
                      : []),
                    ...(JSON.stringify(seen.follows) !== JSON.stringify([A, same ? A : B])
                      ? ["replacement did not own exactly its follow"]
                      : []),
                    ...(seen.stops !== 1
                      ? [`old follow stops ${seen.stops}, expected 1`]
                      : []),
                    ...(JSON.stringify(seen.closed) !== JSON.stringify([A])
                      ? ["close lost original remote target"]
                      : []),
                    ...(seen.statuses !== 0 || seen.errors
                      ? ["old completion polluted replacement notices"]
                      : []),
                    ...(seen.overflow ? ["fixture overflowed horizontally"] : []),
                  ],
                }
              },
            )
            await attempt(rep, { engine, width, name: "console" }, async () => ({}))
          } finally {
            await opened.close()
          }
        }
      }
    })
  },
)
