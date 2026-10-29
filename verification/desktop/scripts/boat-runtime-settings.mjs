#!/usr/bin/env node
/** Real gateway settings edits without a Boat token or a running cloud VM.
 * Both layouts are exercised independently of conversation-list fixtures. */
import { randomUUID } from "node:crypto"
import { mkdirSync } from "node:fs"
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import { panelCredential, startGatewayStack } from "./lib/gateway-stack.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { channelRuntimeSettings } from "./lib/boat-runtime-settings.mjs"
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"

await main(
  {
    name: "boat-runtime-settings",
    summary: "ordinary global VM settings and channel inheritance through the gateway",
    defaults: { engine: "chromium,webkit", layout: "columns,sidebar" },
    options: { evidence: { type: "string" } },
  },
  async ({ options, rep, target: stack }) => {
    if (options.evidence) mkdirSync(options.evidence, { recursive: true })
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        const channel = await stack.client.channels.create({
          requestId: randomUUID(),
          name: `VM settings ${engine} ${layout}`,
          agent: "claude",
          model: "claude-sonnet-5",
        })
        const opened = await openPage(browser, {
          preparePage: (page) => stack.vault.install(page),
          url: new URL("/desktop.html", stack.url).href,
          layout,
          width: 1200,
          height: 900,
          initScripts: [
            [
              gatewayHost,
              {
                endpoint: stack.gateway.url.replace(/^http/, "ws"),
                credential: panelCredential(stack.gateway),
              },
            ],
          ],
        })
        try {
          await attempt(
            rep,
            { engine, layout, name: "default-override-reset" },
            async () => {
              const failures = []
              await channelRuntimeSettings(
                {
                  page: opened.page,
                  client: stack.client,
                  evidence: options.evidence,
                },
                channel,
                failures,
              )
              if (failures.length) throw new Error(failures.join("; "))
            },
          )
        } finally {
          await opened.close()
        }
      }
    })
  },
  (options) =>
    startGatewayStack({ ...options, agent: "claude" }, "boat-runtime-settings", {
      vault: true,
      as: "panel",
      scenario: TEXT_REPLY_SCENARIO,
    }),
)
