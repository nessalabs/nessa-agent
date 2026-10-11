#!/usr/bin/env node
/** Confirmed mode changes through real gateway publications and production consumers (#712). */
import { randomUUID } from "node:crypto"
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { main } from "./lib/run.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { panelTarget, startGatewayStack } from "./lib/gateway-stack.mjs"
import { attachmentVerification, keys, modePublication } from "./lib/selectors.mjs"

const meta = {
  name: "mode-publication",
  summary:
    "confirmed approval mode without transcript output: public read, panel tray and subscribed source revisions",
  defaults: { engine: "chromium,webkit" },
  options: { agent: { type: "string", default: "claude" }, evidence: { type: "string" } },
}
const content = (view) => {
  const { revision: _revision, approvalMode: _approvalMode, ...rest } = view
  return JSON.stringify(rest)
}
await main(
  meta,
  async ({ options, rep, target: stack }) => {
    await withEngines(options, rep, async (engine, browser) => {
      await attempt(rep, { name: "confirmed-mode-consumers", engine }, async () => {
        const id = randomUUID()
        await stack.client.conversation.create({
          conversationId: id,
          agent: options.agent,
        })
        await stack.client.conversation.setApprovalMode(id, "ask")
        const before = await stack.client.conversation.read(id)
        const auto = before.approvalModes.find((choice) => choice.id === "auto")
        const ask = before.approvalModes.find((choice) => choice.id === "ask")
        if (!auto || !ask)
          throw new CannotRun("scripted profile does not offer both Ask and Auto")
        const opened = await openPage(browser, {
          url: new URL(modePublication.page, stack.url).href,
          width: 580,
          height: 600,
          readySelector: modePublication.panel,
          initScripts: [
            [
              (target) => {
                window.__modeTarget = target
              },
              {
                endpoint: stack.gateway.url.replace(/^http/, "ws"),
                credential: stack.credential,
                conversationId: id,
              },
            ],
          ],
        })
        try {
          const { page } = opened
          const initial = await page.evaluate(() => window.__modePublication.snapshot())
          const choose = async (selector, mode) => {
            await page.getByRole(...modePublication.more).click()
            await page.getByRole(...modePublication.approval).click()
            await page.locator(selector).click()
            await page
              .waitForFunction(
                ([selector, mode]) =>
                  document.querySelector(selector)?.textContent === mode &&
                  window.__modePublication.snapshot().sourceMode === mode,
                [modePublication.value, mode],
                { timeout: 10_000 },
              )
              .catch(() => {})
            await page.keyboard.press(keys.escape)
          }
          await choose(modePublication.auto, "auto")
          const after = await stack.client.conversation.read(id)
          const accepted = await page.evaluate(() =>
            window.__modePublication.refreshSource(),
          )
          const failures = []
          if (before.revision === after.revision)
            failures.push("confirmed Auto kept the Ask revision")
          if (after.approvalMode !== "auto")
            failures.push("public read did not publish confirmed Auto")
          if (
            accepted.panelMode !== "auto" ||
            accepted.panelRevision !== after.revision ||
            accepted.sourceRevision <= initial.sourceRevision
          )
            failures.push("production panel/source discarded the confirmed publication")
          if (
            before.messages.length ||
            after.messages.length ||
            accepted.sourceMessages ||
            content(before) !== content(after)
          )
            failures.push(
              "mode change required a transcript or other semantic side effect",
            )
          const repeated = await page.evaluate(() =>
            window.__modePublication.repeat("auto"),
          )
          const repeatedView = await stack.client.conversation.read(id)
          if (
            repeatedView.revision !== after.revision ||
            repeated.panelRevision !== accepted.panelRevision ||
            repeated.sourceRevision !== accepted.sourceRevision
          )
            failures.push("unchanged mode/repeated read advanced publication")
          await page.getByRole(...modePublication.more).click()
          if (!(await page.getByRole(...modePublication.autoLabel).count()))
            failures.push("actual tray does not visibly show confirmed Auto")
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.locator(modePublication.panel).evaluate(async (panel) => {
              await Promise.all(
                panel
                  .getAnimations({ subtree: true })
                  .map((animation) => animation.finished.catch(() => {})),
              )
            })
            await page.locator(modePublication.panel).screenshot({
              path: join(options.shots, `mode-publication-auto-${engine}.png`),
              scale: "css",
            })
          }
          await page.keyboard.press(keys.escape)
          await choose(modePublication.ask, "ask")
          const returned = await stack.client.conversation.read(id)
          const restored = await page.evaluate(() =>
            window.__modePublication.refreshSource(),
          )
          if (
            returned.approvalMode !== "ask" ||
            restored.panelMode !== "ask" ||
            returned.revision === after.revision ||
            restored.sourceRevision <= repeated.sourceRevision ||
            returned.messages.length
          )
            failures.push("return to Ask did not publish without transcript output")
          await page.getByRole(...modePublication.more).click()
          if (!(await page.getByRole(...modePublication.askLabel).count()))
            failures.push("actual tray does not visibly show confirmed Ask")
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.locator(modePublication.panel).evaluate(async (panel) => {
              await Promise.all(
                panel
                  .getAnimations({ subtree: true })
                  .map((animation) => animation.finished.catch(() => {})),
              )
            })
            await page.locator(modePublication.panel).screenshot({
              path: join(options.shots, `mode-publication-${engine}.png`),
              scale: "css",
            })
          }
          await page.keyboard.press(keys.escape)
          const editor = page
            .locator(modePublication.panel)
            .locator(attachmentVerification.editor)
          await editor.fill("Keep this draft")
          await page.waitForFunction(
            () =>
              window.__modePublication.snapshot().draft[0]?.text === "Keep this draft",
          )
          const unavailable = await page.evaluate(() =>
            window.__modePublication.failView(),
          )
          await page.getByRole(...modePublication.more).click()
          const modeRow = page.getByRole(...modePublication.approval)
          if ((await editor.innerText()) !== "Keep this draft")
            failures.push("unavailable view discarded the visible composer draft")
          if (!(await modeRow.isDisabled()))
            failures.push("actual panel retained mode controls after an unavailable view")
          if (
            unavailable.readError !== "unavailable" ||
            unavailable.dispatched !== restored.dispatched ||
            unavailable.draft[0]?.text !== "Keep this draft" ||
            unavailable.declined?.kind !== "view-unavailable"
          )
            failures.push("typed unavailable view admitted a send or discarded its draft")
          if (options.shots)
            await page.locator(modePublication.panel).screenshot({
              path: join(options.shots, `mode-publication-unavailable-${engine}.png`),
              animations: "disabled",
              scale: "css",
            })
          await page.keyboard.press(keys.escape)
          await stack.client.conversation.setApprovalMode(id, "auto")
          const reconciled = await page.evaluate(() =>
            window.__modePublication.recoverView(),
          )
          await page.getByRole(...modePublication.more).click()
          if (
            (await page.getByRole(...modePublication.approval).isDisabled()) ||
            reconciled.readError !== undefined ||
            reconciled.panelMode !== "auto" ||
            reconciled.dispatched !== unavailable.dispatched
          )
            failures.push(
              "confirmed real gateway replacement did not restore the actual panel controls",
            )
          await page.keyboard.press(keys.escape)
          return {
            failures,
            seen: {
              initialMode: initial.panelMode,
              acceptedMode: accepted.panelMode,
              returnedMode: restored.panelMode,
              sourceRevisions: [
                initial.sourceRevision,
                accepted.sourceRevision,
                repeated.sourceRevision,
                restored.sourceRevision,
              ],
              transcriptMessages: returned.messages.length,
              injectedUnavailable: {
                readError: unavailable.readError,
                dispatched: unavailable.dispatched - restored.dispatched,
                declined: unavailable.declined,
                draftKept: unavailable.draft[0]?.text === "Keep this draft",
              },
              reconciledMode: reconciled.panelMode,
            },
          }
        } finally {
          await opened.page
            .evaluate(() => window.__modePublication?.close())
            .catch(() => {})
          await opened.close()
          await stack.client.conversation.close(id)
        }
      })
    })
  },
  async (options) => {
    if (options.mode === "prod")
      throw new CannotRun("mode publication fixture is served by the dev server only")
    return panelTarget(
      await startGatewayStack(options, "mode-publication", {
        as: "panel",
        scripted: "echo",
        evidence: options.evidence,
      }),
    )
  },
)
