#!/usr/bin/env node
/** Real gateway + app-owned native pipe lease, exercised through the unlock UI. */
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import { startGatewayStack, waitFor } from "./lib/gateway-stack.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { VAULT_PASSPHRASE } from "./lib/vault.mjs"
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
await main(
  {
    name: "vault",
    summary: "setup, unlock, reload and app-close encryption lease",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, target: stack }) => {
    await withEngines(options, rep, async (engine, browser) => {
      const opened = await openPage(browser, {
        preparePage: (page) => stack.vault.install(page),
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
      const dialog = page.locator("[data-vault-dialog]")
      const input = () => dialog.getByLabel("Passphrase", { exact: true })
      try {
        await attempt(rep, { engine, name: "session-lifetime" }, async () => {
          await page
            .locator(
              '[data-vault-dialog="setup"], [data-vault-dialog="unlock"], [data-vault-dialog="unavailable"]',
            )
            .waitFor()
          if ((await dialog.getAttribute("data-vault-dialog")) === "unavailable")
            throw new Error(
              `Vault unavailable: ${await dialog.innerText()}; ${JSON.stringify(await stack.vault.command({ operation: "status" }))}`,
            )
          if ((await dialog.getAttribute("data-vault-dialog")) === "setup") {
            if (options.shots)
              await page
                .getByRole("dialog", { name: "Create your Nessa vault", exact: true })
                .screenshot({ path: `${options.shots}/vault-setup-${engine}.png` })
            await input().fill("sixlet")
            if (
              await dialog
                .getByRole("button", { name: "Create vault", exact: true })
                .isEnabled()
            )
              throw new Error("weak passphrase accepted")
            await input().fill(VAULT_PASSPHRASE)
            await dialog
              .getByLabel("Confirm passphrase")
              .fill("different long passphrase")
            await dialog
              .getByRole("button", { name: "Create vault", exact: true })
              .click()
            await dialog.getByRole("alert").filter({ hasText: "don't match" }).waitFor()
            await dialog.getByLabel("Confirm passphrase").fill(VAULT_PASSPHRASE)
            await dialog
              .getByRole("button", { name: "Create vault", exact: true })
              .click()
          } else {
            await input().fill(VAULT_PASSPHRASE)
            await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
          }
          await dialog.waitFor({ state: "hidden" })
          await page.reload()
          await page.locator(".workspace-window").waitFor()
          if ((await stack.client.vault.status()).state !== "unlocked")
            throw new Error("reload lost native lease")
          if (await dialog.isVisible()) throw new Error("reload prompted again")
          await stack.vault.stop()
          if (
            !(await waitFor(
              async () => (await stack.client.vault.status()).state === "locked",
              5000,
            ))
          )
            throw new Error("pipe EOF did not lock vault")
          await page.reload()
          await page
            .locator(
              '[data-vault-dialog="setup"], [data-vault-dialog="unlock"], [data-vault-dialog="unavailable"]',
            )
            .waitFor()
          if ((await dialog.getAttribute("data-vault-dialog")) === "unavailable")
            throw new Error(
              `Vault unavailable: ${await dialog.innerText()}; ${JSON.stringify(await stack.vault.command({ operation: "status" }))}`,
            )
          await input().fill("wrong passphrase for verification")
          await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
          await dialog.getByRole("alert").filter({ hasText: "didn't unlock" }).waitFor()
          if ((await input().inputValue()) !== "")
            throw new Error("submitted passphrase retained in input")
          await dialog.getByRole("button", { name: "About your vault" }).click()
          if ((await dialog.locator("li").count()) !== 3)
            throw new Error("vault help missing")
          if (options.shots)
            await page.screenshot({ path: `${options.shots}/vault-unlock-${engine}.png` })
          await input().fill(VAULT_PASSPHRASE)
          await dialog.getByRole("button", { name: "Unlock", exact: true }).click()
          await dialog.waitFor({ state: "hidden" })
          return {
            reloadPreserved: true,
            eofLocked: true,
            wrongPassphraseRejected: true,
            inputCleared: true,
          }
        })
      } finally {
        await opened.close()
        await stack.vault.stop()
      }
    })
  },
  (options) =>
    startGatewayStack({ ...options, agent: "claude" }, "vault", {
      vault: true,
      initializeVault: false,
      scenario: TEXT_REPLY_SCENARIO,
    }),
)
