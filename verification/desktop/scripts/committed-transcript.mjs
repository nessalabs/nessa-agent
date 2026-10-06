#!/usr/bin/env node
/** Production conversation controls consume committed gateway view states. */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { committedRoles, css } from "./lib/selectors.mjs"

await main(
  {
    name: "committed-transcript",
    summary: "committed history notices and offered controls, including re-enable",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url, mode }) => {
    // An explicit --url is the fixture server the caller named, even beside
    // --mode prod. run-all does not spawn this check under prod.
    if (mode === "prod" && !options.given("url"))
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
          for (const [state, authority, expectedNotices, expectedActions] of [
            ["complete", true, 0, 2],
            ["partial", true, 1, 0],
            ["stale", true, 1, 0],
            ["unknown", true, 1, 0],
            ["not_loaded", true, 1, 0],
            ["complete_empty", true, 0, 0],
            ["complete", false, 0, 0],
            ["complete", true, 0, 2],
          ]) {
            await attempt(
              rep,
              { engine, width, name: `${state}:${authority}` },
              async () => {
                await page.getByRole(...committedRoles.state(state, authority)).click()
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
                await page.getByRole(...committedRoles.questions(limited)).click()
                await page.waitForFunction(
                  ([selector, name]) =>
                    document.querySelector(selector)?.dataset.case === name,
                  [css.committedFixture, `questions:${limited}`],
                )
                const inputs = page.locator(css.committedQuestionInputs)
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
          for (const [state, truncated] of [
            ["not_loaded", false],
            ["partial", false],
            ["stale", false],
            ["unknown", false],
            ["complete", false],
            ["complete_empty", false],
            ["complete_empty", true],
          ]) {
            await attempt(
              rep,
              { engine, width, name: `tab-history:${state}:${truncated}` },
              async () => {
                await page
                  .getByRole(...committedRoles.tabHistory(state, truncated))
                  .click()
                await page.waitForFunction(
                  ([selector, name]) =>
                    document.querySelector(selector)?.dataset.case === name,
                  [css.historyTabConsumer, `${state}:${truncated}`],
                )
                const saved = await page
                  .locator(css.historyTabConsumer)
                  .getAttribute("data-saved-count")
                await page.getByRole(...committedRoles.closeHistory).click()
                await page.waitForFunction(
                  (selector) =>
                    document.querySelector(selector)?.dataset.originalTabPresent ===
                    "false",
                  css.historyTabConsumer,
                )
                const closes = await page
                  .locator(css.historyTabConsumer)
                  .getAttribute("data-close-calls")
                const empty = state === "complete_empty" && !truncated
                return {
                  saved: Number(saved),
                  closes: Number(closes),
                  tabClosed: true,
                  failures: [
                    ...(Number(saved) !== (empty ? 0 : 1)
                      ? ["unconfirmed history lost its saved reference"]
                      : []),
                    ...(Number(closes) !== (empty ? 1 : 0)
                      ? ["tab close dispatched an unconfirmed gateway close"]
                      : []),
                  ],
                }
              },
            )
          }
          await attempt(rep, { engine, width, name: "console" }, async () => ({}))
        } finally {
          await opened.close()
        }
      }
    })
  },
)
