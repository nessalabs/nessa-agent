#!/usr/bin/env node
/** Real source-bound child/grandchild tools and the window's ordinary Stop (#329). */
import { createHash, randomUUID } from "node:crypto"
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"
import { setTimeout as sleep } from "node:timers/promises"
import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, resultOfThrown } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  agentTurn,
  panelTarget,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { main } from "./lib/run.mjs"
import { writeView } from "./lib/scripted-evidence.mjs"
import { css, parentStop } from "./lib/selectors.mjs"

const meta = {
  name: "parent-stop",
  summary:
    "parent Stop cascades to owned descendants by default; explicit settings override preserves them",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { agent: { type: "string", default: "claude" }, evidence: { type: "string" } },
  help: "Needs a built gateway (or MCP_LIVE_NESSA). Signed out, isolated temporary settings, Chromium and WebKit. No live configuration changes.",
}
// Mirrors the boundary's deterministic UUID in conversation_tools::identity.
function childOf(source, request) {
  const hex = createHash("sha256")
    .update(JSON.stringify([source, request, "thread"]))
    .digest("hex")
    .slice(0, 32)
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}
const text = (value) => ({ do: "text", chunks: [value] })
const tool = (name, args) => ({
  do: "tool",
  server: "nessa-agents",
  tool: name,
  arguments: args,
})
const status = (view) => view.messages.at(-1)?.status
const running = (view) => ["running", "queued"].includes(status(view))

await main(
  meta,
  async ({ options, rep }) => {
    const evidence = options.evidence ?? join(tmpdir(), `nessa-parent-stop-${Date.now()}`)
    mkdirSync(evidence, { recursive: true })
    for (const cascade of [true, false]) {
      const policy = cascade ? "default" : "override"
      const dir = join(evidence, policy)
      mkdirSync(dir, { recursive: true })
      const runs = Object.fromEntries(
        options.engines.map((engine) => {
          const parent = randomUUID()
          const child = childOf(parent, "child")
          return [
            engine,
            {
              parent,
              child,
              grandchild: childOf(child, "grandchild"),
              unrelated: randomUUID(),
              marker: `PARENT-${engine}-${policy}`,
            },
          ]
        }),
      )
      const turns = Object.values(runs).flatMap((run) => [
        {
          when: run.marker,
          steps: [
            tool("create_thread", { requestId: "child" }),
            tool("send_message", {
              threadId: run.child,
              message: `CHILD-${run.child}`,
              requestId: "work",
            }),
            text(run.marker),
            { do: "wait-cancel" },
          ],
        },
        {
          when: `CHILD-${run.child}`,
          steps: [
            tool("create_thread", { requestId: "grandchild" }),
            tool("send_message", {
              threadId: run.grandchild,
              message: `GRANDCHILD-${run.grandchild}`,
              requestId: "work",
            }),
            text("CHILD WAITING"),
            { do: "wait-cancel" },
          ],
        },
        {
          when: `GRANDCHILD-${run.grandchild}`,
          steps: [text("GRANDCHILD WAITING"), { do: "wait-cancel" }],
        },
        {
          when: `UNRELATED-${run.unrelated}`,
          steps: [text("UNRELATED WAITING"), { do: "wait-cancel" }],
        },
      ])
      turns.push({ steps: [text("REPORT OR PERSON TURN RAN"), { do: "end" }] })
      const scenario = join(dir, "scenario.json")
      writeFileSync(scenario, JSON.stringify({ turns }))
      let stack
      try {
        stack = await panelTarget(
          await startGatewayStack(options, `parent-stop-${policy}`, {
            as: "panel",
            scenario,
            evidence: dir,
            ...(cascade ? {} : { stopChildrenOnParentStop: false }),
          }),
        )
        await withEngines(options, rep, async (engine, browser) => {
          const run = runs[engine]
          let opened
          try {
            await agentTurn(stack.client, run.unrelated, `UNRELATED-${run.unrelated}`, {
              agent: options.agent,
              create: true,
              seconds: 60,
              ready: (_, turn) => turn.status === "running",
            })
            await agentTurn(stack.client, run.parent, run.marker, {
              agent: options.agent,
              create: true,
              seconds: 60,
              ready: (_, turn) =>
                turn.parts.some((part) => part.text?.includes(run.marker)),
            })
            const descendants = await waitFor(async () => {
              try {
                const child = await stack.client.conversation.read(run.child)
                const grandchild = await stack.client.conversation.read(run.grandchild)
                return running(child) &&
                  running(grandchild) &&
                  grandchild.messages
                    .at(-1)
                    ?.parts.some((part) => part.text?.includes("GRANDCHILD WAITING"))
                  ? { child, grandchild }
                  : null
              } catch {
                return null
              }
            }, 60_000)
            if (!descendants)
              throw new CannotRun(
                "owned child and grandchild did not reach their waiting turns",
              )
            const { conversations } = await stack.client.conversation.list({})
            const title = conversations.find(
              (row) => row.conversationId === run.parent,
            )?.title
            if (!title) throw new CannotRun("the parent has no window title")
            opened = await openPage(browser, {
              url: `${new URL(stack.url).origin}/desktop.html`,
              layout: "columns",
              lines: { engine, layout: "columns" },
              initScripts: [
                [
                  gatewayHost,
                  {
                    endpoint: stack.gateway.url.replace(/^http/, "ws"),
                    credential: stack.credential,
                  },
                ],
              ],
            })
            await opened.page.locator(css.sessionRow, { hasText: title }).first().click()
            await opened.page
              .getByRole(parentStop.stop.role, parentStop.stop)
              .click({ timeout: 30_000 })
            const stopped = await waitFor(async () => {
              const parent = await stack.client.conversation.read(run.parent)
              const child = await stack.client.conversation.read(run.child)
              const grandchild = await stack.client.conversation.read(run.grandchild)
              return status(parent) === "cancelled" &&
                (!cascade ||
                  (status(child) === "cancelled" && status(grandchild) === "cancelled"))
                ? { parent, child, grandchild }
                : null
            }, 30_000)
            if (!stopped)
              throw new Error(
                "Stop did not settle the captured parent/descendant executions within 30s",
              )
            // Observe delivery retries without submitting another person command.
            await sleep(1500)
            const after = await stack.client.conversation.read(run.parent)
            const unrelated = await stack.client.conversation.read(run.unrelated)
            const failures = []
            if (after.messages.length !== 1 || status(after) !== "cancelled")
              failures.push(
                `parent restarted: ${after.messages.length} turns, ${status(after)}`,
              )
            if (!running(unrelated))
              failures.push(`unrelated turn became ${status(unrelated)}`)
            if (!cascade && (!running(stopped.child) || !running(stopped.grandchild)))
              failures.push("override stopped an owned descendant")
            for (const [name, view] of Object.entries({
              ...stopped,
              parent: after,
              unrelated,
            }))
              writeView(dir, `views/${engine}-${name}.json`, view)
            if (options.shots) {
              mkdirSync(options.shots, { recursive: true })
              await opened.page
                .locator(css.pane)
                .first()
                .screenshot({
                  path: join(options.shots, `parent-stop-${engine}-${policy}.png`),
                })
            }
            let releasedReport = null
            if (cascade && failures.length === 0) {
              await agentTurn(stack.client, run.parent, "PERSON CONTINUE", {
                agent: options.agent,
                seconds: 30,
              })
              const continued = await waitFor(async () => {
                const view = await stack.client.conversation.read(run.parent)
                return view.messages.some(
                  (turn) =>
                    turn.userText.startsWith(`[Thread report] Thread ${run.child} `) &&
                    turn.status === "completed",
                )
                  ? view
                  : null
              }, 30_000)
              releasedReport = Boolean(continued)
              if (!continued)
                failures.push(
                  "a later person command did not release the retained child report",
                )
              else writeView(dir, `views/${engine}-continued.json`, continued)
            }
            rep.add({
              name: policy,
              engine,
              layout: "columns",
              seen: {
                parentTurns: after.messages.length,
                parent: status(after),
                child: status(stopped.child),
                grandchild: status(stopped.grandchild),
                unrelated: status(unrelated),
                observationMs: 1500,
                releasedReport,
              },
              failures,
            })
          } catch (error) {
            rep.add(resultOfThrown({ name: policy, engine, layout: "columns" }, error))
          } finally {
            await opened?.close()
            for (const id of [run.unrelated, run.child, run.grandchild])
              await stack.client.stopThread(id).catch(() => {})
          }
        })
      } finally {
        await stack?.close()
      }
    }
  },
  async (options) => ({ mode: options.mode, close: async () => {} }),
)
