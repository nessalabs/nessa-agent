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
import { hasAgentSource } from "./lib/stress-parent-stop.mjs"
import { css, composerControls } from "./lib/selectors.mjs"

const meta = {
  name: "parent-stop",
  summary:
    "parent Stop cascades to owned descendants by default; explicit settings override preserves them",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { agent: { type: "string", default: "claude" }, evidence: { type: "string" } },
  help: "Needs a built gateway (or MCP_LIVE_NESSA). Signed out, isolated temporary settings, Chromium and WebKit. No live configuration changes.",
}
// Mirrors the boundary's deterministic UUID in conversation_tools::identity.
function identity(source, request, purpose = "thread") {
  const hex = createHash("sha256")
    .update(JSON.stringify([source, request, purpose]))
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
const status = (view, execution) =>
  (execution
    ? view.messages.find((turn) => turn.executionId === execution)
    : view.messages.at(-1)
  )?.status
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
          const child = identity(parent, "child")
          return [
            engine,
            {
              parent,
              child,
              grandchild: identity(child, "grandchild"),
              unrelated: randomUUID(),
              unownedExecution: identity(parent, "unowned", "execution"),
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
            ...(cascade
              ? [
                  tool("send_message", {
                    threadId: run.child,
                    message: `SEED-${run.child}`,
                    requestId: "work",
                  }),
                ]
              : []),
            tool("send_message", {
              threadId: run.unrelated,
              message: `UNRELATED-${run.unrelated}`,
              requestId: "unowned",
            }),
            text(run.marker),
            { do: "wait-cancel" },
          ],
        },
        {
          when: `SEED-${run.child}`,
          steps: [text("CHILD SEED COMPLETED"), { do: "end" }],
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
            const unrelatedStart = await agentTurn(
              stack.client,
              run.unrelated,
              `UNRELATED-${run.unrelated}`,
              {
                agent: options.agent,
                create: true,
                seconds: 60,
                ready: (_, turn) => turn.status === "running",
              },
            )
            const unrelatedExecution = unrelatedStart.turn.executionId
            const parentStart = await agentTurn(stack.client, run.parent, run.marker, {
              agent: options.agent,
              create: true,
              seconds: 60,
              ready: (_, turn) =>
                turn.parts.some((part) => part.text?.includes(run.marker)),
            })
            // Agent-origin input cannot relay agent tools. A genuine person
            // starts the child's grandchild-creation turn in the owned lifetime.
            const childStart = await agentTurn(
              stack.client,
              run.child,
              `CHILD-${run.child}`,
              {
                agent: options.agent,
                seconds: 60,
                ready: (_, turn) =>
                  turn.parts.some((part) => part.text?.includes("CHILD WAITING")),
              },
            )
            if (childStart.turn.agent)
              throw new Error("the child setup turn was not person-authored")
            const parentExecution = parentStart.turn.executionId
            const childExecution = childStart.turn.executionId
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
            const grandchildExecution = descendants.grandchild.messages.at(-1).executionId
            if (
              !hasAgentSource(descendants.grandchild.messages.at(-1), {
                conversationId: run.child,
                executionId: childExecution,
              })
            )
              throw new Error("the grandchild lost its exact child-turn author")
            const seed = childStart.view.messages.find(
              (turn) => turn.userText === `SEED-${run.child}`,
            )
            if (
              cascade &&
              (seed?.status !== "completed" ||
                !hasAgentSource(seed, {
                  conversationId: run.parent,
                  executionId: parentExecution,
                }))
            )
              throw new Error(
                "the completed child seed lost its exact parent-turn author",
              )
            const beforeStop = cascade
              ? await waitFor(async () => {
                  const view = await stack.client.conversation.read(run.parent)
                  return seed &&
                    view.messages.some(
                      (turn) =>
                        hasAgentSource(turn, {
                          conversationId: run.child,
                          executionId: seed.executionId,
                        }) && turn.status === "queued",
                    )
                    ? view
                    : null
                }, 30_000)
              : await stack.client.conversation.read(run.parent)
            if (!beforeStop || beforeStop.messages.length !== (cascade ? 2 : 1))
              throw new CannotRun(
                "the exact parent and optional seed report were not ready before Stop",
              )
            const seedReportExecution = cascade
              ? beforeStop.messages.find((turn) =>
                  hasAgentSource(turn, {
                    conversationId: run.child,
                    executionId: seed.executionId,
                  }),
                )?.executionId
              : null
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
              .getByRole(...composerControls.stop)
              .click({ timeout: 30_000 })
            const stopped = await waitFor(async () => {
              const parent = await stack.client.conversation.read(run.parent)
              const child = await stack.client.conversation.read(run.child)
              const grandchild = await stack.client.conversation.read(run.grandchild)
              return status(parent, parentExecution) === "cancelled" &&
                (!cascade ||
                  (status(child, childExecution) === "cancelled" &&
                    status(grandchild, grandchildExecution) === "cancelled"))
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
            const childAfter = await stack.client.conversation.read(run.child)
            const grandchildAfter = await stack.client.conversation.read(run.grandchild)
            const unrelated = await stack.client.conversation.read(run.unrelated)
            const failures = []
            if (
              after.messages.length !== beforeStop.messages.length ||
              status(after, parentExecution) !== "cancelled" ||
              after.messages.some((turn) => turn.status !== "cancelled")
            )
              failures.push(
                `parent restarted: ${after.messages.length} turns, ${status(after)}`,
              )
            const unrelatedActive = unrelated.messages.find(
              (turn) => turn.executionId === unrelatedExecution,
            )
            if (unrelatedActive?.status !== "running")
              failures.push(
                `unrelated active turn became ${unrelatedActive?.status ?? "missing"}`,
              )
            const unrelatedQueued = unrelated.pending.filter(
              (turn) => turn.executionId === run.unownedExecution,
            ).length
            if (unrelatedQueued !== 1)
              failures.push(
                `unowned report-watch target retained ${unrelatedQueued} of its queued turns`,
              )
            if (
              !cascade &&
              (status(childAfter, childExecution) !== "running" ||
                status(grandchildAfter, grandchildExecution) !== "running")
            )
              failures.push("override stopped an owned descendant")
            if (
              cascade &&
              [childAfter, grandchildAfter].some(
                (view) =>
                  !view.queueComplete ||
                  view.messages.some(
                    (turn) => turn.status === "running" || turn.status === "queued",
                  ),
              )
            )
              failures.push("a report restarted or queued work in a stopped descendant")
            for (const [name, view] of Object.entries({
              ...stopped,
              parent: after,
              child: childAfter,
              grandchild: grandchildAfter,
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
                  scale: "css",
                })
            }
            let releasedReport = null
            if (cascade && failures.length === 0) {
              try {
                await stack.client.conversation.submit(
                  stack.client.conversation.prepare(run.parent, "PERSON CONTINUE"),
                )
              } catch (error) {
                throw new Error(
                  `later person submission refused: code=${error.code ?? "unknown"}, rpc=${error.cause?.code ?? "unknown"}, uncertain=${error.uncertain}`,
                  { cause: error },
                )
              }
              const continued = await waitFor(async () => {
                const view = await stack.client.conversation.read(run.parent)
                return view.messages.some(
                  (turn) =>
                    turn.userText.startsWith(`[Thread report] Thread ${run.child} `) &&
                    turn.status === "completed" &&
                    turn.executionId !== seedReportExecution &&
                    hasAgentSource(turn, {
                      conversationId: run.child,
                      executionId: seed.executionId,
                    }),
                ) && status(view, seedReportExecution) === "cancelled"
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
                parent: status(after, parentExecution),
                child: status(childAfter, childExecution),
                grandchild: status(grandchildAfter, grandchildExecution),
                childAuthor: "person",
                unrelated: unrelatedActive?.status ?? null,
                unrelatedQueued,
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
