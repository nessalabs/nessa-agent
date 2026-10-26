#!/usr/bin/env node
/** Settings over the real gateway and private inbox, with a loopback GitHub
 * device-flow/repository provider. The cloud relay itself is verified by the
 * bundled Worker tests; this check never authorizes a real GitHub account. */
import { mkdtempSync, mkdirSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { DatabaseSync } from "node:sqlite"
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import app from "../../../protocol/defaults/github-app.json" with { type: "json" }
import { startFakeGithub } from "./lib/fake-github.mjs"
import { startGatewayStack, waitFor } from "./lib/gateway-stack.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { attempt } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys } from "./lib/selectors.mjs"

await main(
  {
    name: "github-live-events",
    summary: "existing device sign-in, live opt-in, failure and disconnect",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, target }) => {
    const stack = target
    await withEngines(options, rep, async (engine, browser) => {
      const opened = await openPage(browser, {
        url: new URL("/desktop.html", stack.url).href,
        layout: "columns",
        width: 1000,
        height: 800,
        initScripts: [
          [
            gatewayHost,
            {
              endpoint: stack.gateway.url.replace(/^http/, "ws"),
              credential: stack.token(),
            },
          ],
        ],
      })
      const { page } = opened
      try {
        await attempt(rep, { engine, name: "opt-in-failure-disconnect" }, async () => {
          await need(page, css.pane, "workspace")
          await page.keyboard.press(keys.settings)
          await need(page, css.settings, "settings")
          await page
            .locator(css.settingsCategory, { hasText: "Connections" })
            .first()
            .click()
          await page.locator(css.settingsTab, { hasText: "Accounts" }).first().click()
          const row = page.locator(css.github)
          await row.getByRole("button", { name: "Connect…", exact: true }).click()
          if (
            !(await waitFor(
              async () => (await row.getAttribute("data-github-state")) === "connecting",
              15000,
            ))
          )
            throw new Error("device code not shown")
          stack.github.approve()
          if (
            !(await waitFor(
              async () => (await row.getAttribute("data-github-state")) === "connected",
              15000,
            ))
          )
            throw new Error("sign-in not connected")
          const liveRow = page.locator(css.githubLive)
          const toggle = liveRow.getByRole("switch", {
            name: "Enable live GitHub events",
          })
          if ((await toggle.getAttribute("aria-checked")) !== "false")
            throw new Error("live events not initially off")
          if (await row.locator(css.githubLive).count())
            throw new Error("live events are nested in the account row")
          const help = liveRow.getByRole("button", {
            name: "About live GitHub events",
            exact: true,
          })
          const popup = page.getByRole("dialog", {
            name: "About live GitHub events",
            exact: true,
          })
          const helpSizes = []
          for (const width of [1000, 600]) {
            await page.setViewportSize({ width, height: 800 })
            await help.click()
            await popup.waitFor({ state: "visible" })
            await page.waitForFunction(
              () => {
                const button = document.querySelector(
                  "[data-github-live] button[popovertarget]",
                )
                const target =
                  button && document.getElementById(button.getAttribute("popovertarget"))
                if (!button || !target) return false
                const anchor = button.getBoundingClientRect(),
                  box = target.getBoundingClientRect()
                return (
                  Math.abs(box.right - anchor.right) < 9 &&
                  (Math.abs(box.top - anchor.bottom - 6) < 3 ||
                    Math.abs(anchor.top - box.bottom - 6) < 3)
                )
              },
              css.githubLiveHelp,
              { timeout: 5000 },
            )
            if (
              (await popup.locator("li").count()) !== 5 ||
              !(await popup.textContent()).includes("Cloudflare relay")
            )
              throw new Error("bulleted relay disclosure missing")
            const bounds = await popup.boundingBox()
            if (
              !bounds ||
              bounds.x < 0 ||
              bounds.y < 0 ||
              bounds.x + bounds.width > width + 1 ||
              bounds.y + bounds.height > 801
            )
              throw new Error("help popup leaves viewport")
            helpSizes.push({ width, popup: bounds })
            await page.keyboard.press("Escape")
            await popup.waitFor({ state: "hidden" })
            if (!(await help.evaluate((node) => document.activeElement === node)))
              throw new Error("Escape did not return focus to help")
            if (!(await page.locator(css.settings).isVisible()))
              throw new Error("Escape also closed Settings")
          }
          await page.setViewportSize({ width: 1000, height: 800 })
          await toggle.click()
          if (
            !(await waitFor(
              async () => (await toggle.getAttribute("aria-checked")) === "true",
              10000,
            ))
          )
            throw new Error("enable not confirmed")
          const on = await stack.client.github.status()
          if (on.liveEvents?.repositories !== 1)
            throw new Error("repository count not persisted")
          const rect = await toggle.boundingBox()
          const detail = await liveRow.boundingBox()
          if (
            !rect ||
            !detail ||
            detail.height > 100 ||
            rect.width < 24 ||
            rect.height < 16
          )
            throw new Error("switch geometry invalid")
          await page.reload()
          await need(page, css.pane, "workspace after reload")
          await page.keyboard.press(keys.settings)
          await need(page, css.settings, "settings")
          await page
            .locator(css.settingsCategory, { hasText: "Connections" })
            .first()
            .click()
          await page.locator(css.settingsTab, { hasText: "Accounts" }).first().click()
          if (
            !(await waitFor(
              async () => (await toggle.getAttribute("aria-checked")) === "true",
              10000,
            ))
          )
            throw new Error("reload lost opt-in")
          if (options.shots) {
            mkdirSync(options.shots, { recursive: true })
            await liveRow
              .locator("..")
              .screenshot({ path: join(options.shots, `github-live-${engine}.png`) })
            await help.click()
            await popup.waitFor({ state: "visible" })
            await popup.screenshot({
              path: join(options.shots, `github-live-help-${engine}.png`),
            })
            await page.keyboard.press("Escape")
          }
          await toggle.click()
          if (
            !(await waitFor(
              async () => (await toggle.getAttribute("aria-checked")) === "false",
              10000,
            ))
          )
            throw new Error("disable not confirmed")
          stack.github.repositories(1, true)
          await toggle.click()
          if (
            !(await waitFor(
              async () =>
                (await row.textContent()).includes("Couldn't change live GitHub events"),
              10000,
            ))
          )
            throw new Error("provider failure hidden")
          if ((await toggle.getAttribute("aria-checked")) !== "false")
            throw new Error("failed action changed switch")
          stack.github.repositories(1, false)
          await toggle.click()
          if (
            !(await waitFor(
              async () => (await toggle.getAttribute("aria-checked")) === "true",
              10000,
            ))
          )
            throw new Error("retry did not recover")
          const db = new DatabaseSync(
            join(
              stack.gateway.directory,
              "dev",
              "instances",
              "github-live-events",
              "github-relay",
              "inbox.sqlite",
            ),
          )
          try {
            db.exec(
              "CREATE TRIGGER refuse_live_cleanup BEFORE INSERT ON binding_audit WHEN NEW.cause='live_events_disabled' BEGIN SELECT RAISE(ABORT,'test audit unavailable'); END;",
            )
            await row.getByRole("button", { name: "Disconnect", exact: true }).click()
            if (
              !(await waitFor(
                async () =>
                  (await row.textContent()).includes(
                    "Couldn't change live GitHub events",
                  ),
                10000,
              ))
            )
              throw new Error("cleanup failure not shown")
            const held = await stack.client.github.status()
            if (held.state !== "connected" || !held.liveEvents?.enabled)
              throw new Error("failed cleanup forgot sign-in or selection")
          } finally {
            db.exec("DROP TRIGGER refuse_live_cleanup")
            db.close()
          }
          await row.getByRole("button", { name: "Disconnect", exact: true }).click()
          if (
            !(await waitFor(
              async () => (await row.getAttribute("data-github-state")) === "signed-out",
              10000,
            ))
          )
            throw new Error("disconnect not confirmed")
          const off = await stack.client.github.status()
          if (off.liveEvents?.enabled || off.liveEvents?.repositories !== 0)
            throw new Error("disconnect retained live selection")
          await row.getByRole("button", { name: "Connect…", exact: true }).click()
          if (
            !(await waitFor(
              async () => (await row.getAttribute("data-github-state")) === "connecting",
              10000,
            ))
          )
            throw new Error("second sign-in code not shown")
          stack.github.approve()
          if (
            !(await waitFor(
              async () => (await row.getAttribute("data-github-state")) === "connected",
              10000,
            ))
          )
            throw new Error("second sign-in did not finish")
          const raced = await Promise.allSettled([
            stack.client.github.liveEvents(true),
            stack.client.github.disconnect(),
          ])
          if (raced[1].status !== "fulfilled")
            throw new Error("concurrent sign-out failed")
          const racedOff = await stack.client.github.status()
          if (racedOff.state !== "signed_out" || racedOff.liveEvents?.repositories !== 0)
            throw new Error("concurrent enable revived a disconnected account")
          return {
            helpSizes,
            cleanupFailureKeptSignIn: true,
            concurrentDisconnectPurged: true,
            repositories: on.liveEvents.repositories,
            afterDisconnect: off.liveEvents.repositories,
            reloadPreserved: true,
            failedEnablePreserved: true,
            switch: rect,
            row: detail,
          }
        })
      } finally {
        await opened.close()
        stack.github.repositories(1, false)
        await stack.client.github.disconnect()
      }
    })
  },
  async (options) => {
    const directory = mkdtempSync(join(tmpdir(), "nessa-live-github-"))
    const github = await startFakeGithub({ clientId: app.clientId, slug: app.slug })
    let stack
    try {
      stack = await startGatewayStack(
        { ...options, agent: "claude" },
        "github-live-events",
        {
          stage: "dev",
          scenario: TEXT_REPLY_SCENARIO,
          environment: { NESSA_GITHUB_APP: github.app(join(directory, "app.json")) },
        },
      )
    } catch (error) {
      await github.close()
      rmSync(directory, { recursive: true, force: true })
      throw error
    }
    return {
      ...stack,
      github,
      close: async () => {
        try {
          await stack.close()
        } finally {
          await github.close()
          rmSync(directory, { recursive: true, force: true })
        }
      },
    }
  },
)
