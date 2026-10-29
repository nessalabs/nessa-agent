#!/usr/bin/env node
/** Production conversation controls consume committed gateway view states. */
import { mkdirSync } from "node:fs"
import { resolve } from "node:path"
import { startPreview, target, repoRoot } from "./lib/server.mjs"
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { committedRoles, css } from "./lib/selectors.mjs"

await main(
  {
    name: "committed-transcript",
    summary: "committed history notices and offered controls, including re-enable",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
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
          await attempt(
            rep,
            { engine, width, name: "question-target-identities" },
            async () => {
              const first = "00000000-0000-4000-8000-000000000001"
              const second = "00000000-0000-4000-8000-000000000002"
              const consumer = page.locator(css.questionTargetConsumer)
              const active = consumer.locator(css.questionTargetActive)
              const fields = page.locator(css.questionTargetInputs)
              const answer = active.getByRole(...committedRoles.questionAnswer)
              const skip = active.getByRole(...committedRoles.questionSkip)
              await page.waitForFunction(
                (selector) => document.querySelectorAll(selector).length === 3,
                css.questionTargetInputs,
              )
              const fieldCount = await fields.count()
              await fields.nth(1).check()
              const firstSelected = await fields.evaluateAll(
                (nodes) => nodes.filter((node) => node.checked).length,
              )
              const unrelatedSelected = await page
                .locator(css.committedQuestionInputs)
                .evaluateAll((nodes) => nodes.map((node) => node.checked))
              await page.getByRole(...committedRoles.questionConversation(true)).click()
              await page.waitForFunction(
                ([selector, id]) =>
                  document.querySelector(selector)?.dataset.questionTargetActive === id,
                [css.questionTargetActive, second],
              )
              const inheritedSelected = await fields.evaluateAll(
                (nodes) => nodes.filter((node) => node.checked).length,
              )
              const blockedEmpty = await answer.isDisabled()
              await fields.nth(2).check()
              await answer.click()
              await page.waitForFunction(
                (selector) =>
                  JSON.parse(document.querySelector(selector)?.dataset.requests ?? "[]")
                    .length === 1,
                css.questionTargetConsumer,
              )
              await page.getByRole(...committedRoles.questionConversation(false)).click()
              await page.waitForFunction(
                ([selector, id]) =>
                  document.querySelector(selector)?.dataset.questionTargetActive === id,
                [css.questionTargetActive, first],
              )
              await skip.click()
              await page.waitForFunction(
                (selector) =>
                  JSON.parse(document.querySelector(selector)?.dataset.requests ?? "[]")
                    .length === 2,
                css.questionTargetConsumer,
              )
              const requests = JSON.parse(await consumer.getAttribute("data-requests"))
              if (options.shots && engine === "chromium" && width === 600) {
                mkdirSync(options.shots, { recursive: true })
                await consumer.screenshot({
                  path: resolve(options.shots, "question-targets.png"),
                  scale: "css",
                })
              }
              return {
                fieldCount,
                firstSelected,
                unrelatedSelected,
                inheritedSelected,
                blockedEmpty,
                requests,
                failures: [
                  ...(fieldCount !== 3 ? ["published question fields changed"] : []),
                  ...(firstSelected !== 1 ? ["original choice did not select"] : []),
                  ...(JSON.stringify(unrelatedSelected) !== "[false,true,false]"
                    ? ["equal field keys changed another ask’s retained middle selection"]
                    : []),
                  ...(inheritedSelected !== 0 || !blockedEmpty
                    ? ["another conversation inherited the original answer draft"]
                    : []),
                  ...(requests.length !== 2
                    ? ["answer/decline action count mismatch"]
                    : []),
                  ...(requests[0]?.conversationId !== second ||
                  requests[1]?.conversationId !== first
                    ? ["action conversation identities mixed"]
                    : []),
                  ...(requests.some(
                    (request) =>
                      request.executionId !== "exact-live-execution" ||
                      request.questionId !== "retained-question" ||
                      request.question !== "abcdef0123456789abcdef0123456789",
                  )
                    ? ["action turn/question identities mixed"]
                    : []),
                  ...(JSON.stringify(requests[0]?.choices) !==
                    JSON.stringify([{ key: "environment_2", values: ["staging"] }]) ||
                  requests[1]?.choices !== null
                    ? ["answer choice or null decline changed"]
                    : []),
                ],
              }
            },
          )
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
                const empty = state === "complete_empty" && !truncated
                await page.getByRole(...committedRoles.closeHistory).click()
                await page.waitForFunction(
                  (selector) =>
                    document.querySelector(selector)?.dataset.originalTabPresent ===
                    "false",
                  css.historyTabConsumer,
                  { timeout: 5_000 },
                )
                const tabPresent =
                  (await page
                    .locator(css.historyTabConsumer)
                    .getAttribute("data-original-tab-present")) === "true"
                const closes = await page
                  .locator(css.historyTabConsumer)
                  .getAttribute("data-close-calls")
                return {
                  saved: Number(saved),
                  closes: Number(closes),
                  tabPresent,
                  tabClosed: !tabPresent,
                  failures: [
                    ...(Number(saved) !== (empty ? 0 : 1)
                      ? ["unconfirmed history lost its saved reference"]
                      : []),
                    ...(Number(closes) !== (empty ? 1 : 0)
                      ? ["tab close dispatched an unconfirmed gateway close"]
                      : []),
                    ...(tabPresent
                      ? ["closed history tab remained in the local conversations"]
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
  (options) =>
    options.mode === "prod" && !options.url
      ? startPreview(
          options,
          {},
          resolve(repoRoot, "verification/conversation/vite.config.ts"),
        )
      : target(options),
)
