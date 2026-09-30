#!/usr/bin/env node
/** Production conversation controls consume committed gateway view states. */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

await main(
  {
    name: "committed-transcript",
    summary: "committed history notices and offered controls, including re-enable",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url, mode }) => {
    if (mode === "prod")
      throw new CannotRun(
        "the production-component fixture requires a Vite development server or an explicitly supplied fixture-serving URL",
      )
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of options.quick ? [600] : [360, 600, 1440]) {
        const opened = await openPage(browser, {
          url: new URL("/verification/conversation/fixture.html", url).href,
          readySelector: css.committedFixture,
          width,
          mac: false,
        })
        const { page } = opened
        try {
          for (const [label, state, authority, expectedNotices, expectedActions] of [
            ["complete", "complete", true, 0, 2],
            ["partial", "partial", true, 1, 0],
            ["stale", "stale", true, 1, 0],
            ["unknown", "unknown", true, 1, 0],
            ["not_loaded", "not_loaded", true, 1, 0],
            ["complete_empty", "complete_empty", true, 0, 0],
            ["complete without live attachment", "complete", false, 0, 0],
            ["complete", "complete", true, 0, 2],
          ]) {
            await attempt(
              rep,
              { engine, width, name: `${state}:${authority}` },
              async () => {
                await page.getByRole("button", { name: label, exact: true }).click()
                await page.waitForFunction(
                  ([selector, name]) =>
                    document.querySelector(selector)?.dataset.case === name,
                  [css.committedFixture, `${state}:${authority}`],
                )
                const notices = await page.locator(css.committedNotice).count()
                const enabled = await page
                  .locator(css.committedActions)
                  .evaluateAll(
                    (buttons) => buttons.filter((button) => !button.disabled).length,
                  )
                const bounds = await page
                  .locator(css.committedControls)
                  .evaluate((element) => ({
                    width: element.getBoundingClientRect().width,
                    scrollWidth: element.scrollWidth,
                    clientWidth: element.clientWidth,
                  }))
                return {
                  notices,
                  enabled,
                  bounds,
                  failures: [
                    ...(notices !== expectedNotices
                      ? [`notices ${notices}, expected ${expectedNotices}`]
                      : []),
                    ...(enabled !== expectedActions
                      ? [`enabled ${enabled}, expected ${expectedActions}`]
                      : []),
                    ...(bounds.scrollWidth > bounds.clientWidth + 1
                      ? ["controls overflow horizontally"]
                      : []),
                  ],
                }
              },
            )
          }
          for (const [step, limited] of [false, true, false].entries()) {
            await attempt(
              rep,
              { engine, width, name: `questions:${limited}` },
              async () => {
                await page
                  .getByRole("button", {
                    name: limited ? "questions with display limit" : "questions",
                    exact: true,
                  })
                  .click()
                await page.waitForFunction(
                  ([selector, name]) =>
                    document.querySelector(selector)?.dataset.case === name,
                  [css.committedFixture, `questions:${limited}`],
                )
                const inputs = page.locator(
                  `${css.committedQuestions} input[type="radio"]`,
                )
                if (step === 0) await inputs.nth(1).check()
                const selected = await inputs.evaluateAll((nodes) =>
                  nodes.map((node) => node.checked),
                )
                const notice = await page.locator(css.committedLimitNotice).innerText()
                const count = await inputs.count()
                const bounds = await page
                  .locator(css.committedQuestions)
                  .evaluate((node) => ({
                    scrollWidth: node.scrollWidth,
                    clientWidth: node.clientWidth,
                  }))
                return {
                  count,
                  selected,
                  notice,
                  bounds,
                  failures: [
                    ...(count !== 3 ? [`question siblings ${count}, expected3`] : []),
                    ...(JSON.stringify(selected) !== "[false,true,false]"
                      ? ["question keys lost selection"]
                      : []),
                    ...(limited !== notice.includes("Use Stop")
                      ? ["display limit notice mismatch"]
                      : []),
                    ...(bounds.scrollWidth > bounds.clientWidth + 1
                      ? ["questions overflow horizontally"]
                      : []),
                  ],
                }
              },
            )
          }
          await attempt(rep, { engine, width, name: "console" }, async () => ({
            failures: opened.errors,
          }))
        } finally {
          await opened.close()
        }
      }
    })
  },
)
