#!/usr/bin/env node
/** Actual Messages catalogue updates from a second gateway caller, with no remount or polling (#722). */
import { randomUUID } from "node:crypto"
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { main } from "./lib/run.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { agentTurn, panelTarget, startGatewayStack } from "./lib/gateway-stack.mjs"
import { panelListFollow } from "./lib/selectors.mjs"
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"

const meta = {
  name: "panel-list-follow",
  summary:
    "actual panel Messages follows explicit active/archived catalogues and releases both streams",
  defaults: { engine: "chromium,webkit" },
  options: { agent: { type: "string", default: "claude" }, evidence: { type: "string" } },
}
await main(
  meta,
  async ({ options, rep, target: stack }) => {
    await withEngines(options, rep, async (engine, browser) => {
      await attempt(rep, { name: "external-catalogue-and-cleanup", engine }, async () => {
        const ids = [randomUUID(), randomUUID(), randomUUID()]
        const opened = await openPage(browser, {
          url: new URL(panelListFollow.page, stack.url).href,
          width: 520,
          height: 640,
          readySelector: panelListFollow.panel,
          initScripts: [
            [
              (target) => {
                window.__panelListTarget = target
              },
              {
                endpoint: stack.gateway.url.replace(/^http/, "ws"),
                credential: stack.credential,
              },
            ],
          ],
        })
        try {
          const { page } = opened
          const wait = async (test, arg) =>
            page.waitForFunction(test, arg, { timeout: 15000 })
          const snapshot = () => page.evaluate(() => window.__panelListFollow.snapshot())
          await wait(() => window.__panelListFollow.snapshot().rows !== null)
          const initial = await snapshot()
          const send = async (id, text) => {
            const result = await agentTurn(stack.client, id, text, {
              agent: options.agent,
              create: true,
              seconds: 30,
            })
            if (result.turn.status !== "completed")
              throw new Error(`scripted turn ended ${result.turn.status}`)
          }
          await send(ids[0], "Panel catalogue first")
          const row = page
            .locator(panelListFollow.row)
            .filter({ hasText: "Panel catalogue first" })
          await row.waitFor({ state: "visible" })
          await page.evaluate((id) => window.__panelListFollow.hold(id), ids[0])
          const live = await snapshot()
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await page.locator(panelListFollow.panel).screenshot({
              path: join(options.shots, `panel-list-follow-live-${engine}.png`),
              scale: "css",
            })
          }
          await stack.client.conversation.archive(ids[0])
          await wait(
            (id) => window.__panelListFollow.snapshot().archivedIds.includes(id),
            ids[0],
          )
          await row.waitFor({ state: "hidden" })
          const archived = await snapshot()
          await stack.client.conversation.unarchive(ids[0])
          await wait(
            (id) => !window.__panelListFollow.snapshot().archivedIds.includes(id),
            ids[0],
          )
          await row.waitFor({ state: "visible" })
          await stack.client.conversation.delete(ids[0])
          await wait(
            (id) => !window.__panelListFollow.snapshot().rows?.includes(id),
            ids[0],
          )
          // An externally deleted held tab is not inferred deleted from catalogue omission.
          // Its local row can stay until that tab learns the typed deletion itself.
          await page.evaluate(() => window.__panelListFollow.failArchived())
          await send(ids[1], "Panel catalogue second")
          await page
            .locator(panelListFollow.row)
            .filter({ hasText: "Panel catalogue second" })
            .waitFor({ state: "visible" })
          const stale = await snapshot()
          if (stale.failure !== "unavailable")
            throw new Error(
              "the successful active half cleared the archived half's stale failure",
            )
          await page
            .getByRole("status")
            .filter({ hasText: "The list could not be refreshed" })
            .waitFor({ state: "visible" })
          if (options.shots)
            await page.locator(panelListFollow.panel).screenshot({
              path: join(options.shots, `panel-list-follow-stale-${engine}.png`),
              scale: "css",
            })
          await page.evaluate(() => window.__panelListFollow.hide())
          await wait(() => !window.__panelListFollow.snapshot().following)
          const hidden = await snapshot()
          await send(ids[2], "Panel catalogue while hidden")
          const afterHidden = await snapshot()
          if (JSON.stringify(hidden.rows) !== JSON.stringify(afterHidden.rows))
            throw new Error("closed Messages accepted an external frame")
          await page.evaluate(() => window.__panelListFollow.show())
          await wait(
            (id) => window.__panelListFollow.snapshot().rows?.includes(id),
            ids[2],
          )
          const restored = await snapshot()
          const failures = []
          if (initial.opens !== 2 || live.opens !== 2 || archived.opens !== 2)
            failures.push(
              "normal external catalogue changes remounted or restarted the pair",
            )
          if (hidden.stops !== 2 || restored.opens !== 4)
            failures.push(
              "leaving/reopening did not release exactly its active and archived pair",
            )
          if (restored.failure !== null)
            failures.push("new paired frames retained the old outage")
          if (restored.oneShotReads !== 0)
            failures.push("a live Messages follow asked through the one-shot list path")
          await opened.settleRequests()
          if (opened.errors.length) failures.push(...opened.errors)
          return {
            failures,
            detail: {
              initial,
              live,
              archived,
              controlledArchivedPortFailure: stale,
              hidden,
              restored,
              externalCaller: true,
              provider: "scripted",
              catalogueHalvesAtomic: false,
            },
          }
        } finally {
          await opened.page
            .evaluate(() => window.__panelListFollow?.close())
            .catch(() => {})
          await opened.close()
          for (const id of ids) await stack.client.conversation.delete(id).catch(() => {})
        }
      })
    })
  },
  async (options) => {
    if (options.mode === "prod")
      throw new CannotRun("panel list fixture is served by the dev server only")
    return panelTarget(
      await startGatewayStack(options, "panel-list-follow", {
        as: "panel",
        scenario: TEXT_REPLY_SCENARIO,
        evidence: options.evidence,
      }),
    )
  },
)
