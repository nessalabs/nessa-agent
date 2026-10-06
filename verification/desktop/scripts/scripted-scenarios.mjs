#!/usr/bin/env node
/**
 * The window scenario through a real gateway (#510): a permission the person
 * allows, a turn that fails after it has spoken, and a turn that waits until
 * the conversation is closed. The scripted agent runs
 * `scripts/mcp-test-server/scenarios/window.json`. The strings the steps send
 * and look for are read from that file (`scenarioScript`), so the file is
 * their owner.
 *
 * Signed out. One conversation per engine and layout for the permission and
 * the failure. A prompt that returns an error retires that provider session,
 * so the cancel turn is a new conversation: closing the failed one would not
 * be what ends it. Chromium and WebKit. `--mode prod` previews a production
 * build; the fake host answers the endpoint, so the preview does not need the
 * dev server's proxy. The page's console and request lines stay in this
 * check's results, recorded by `browser.mjs`.
 */
import { randomUUID } from "node:crypto"
import { mkdirSync, readFileSync } from "node:fs"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import {
  parseScenario,
  scenarioScript,
} from "../../../scripts/mcp-test-server/scripted-scenario.mjs"
import { WINDOW_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, chosen, log, resultOfThrown } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  agentTurn,
  panelTarget,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { main } from "./lib/run.mjs"
import { writeView } from "./lib/scripted-evidence.mjs"
import { css, offeredLabel } from "./lib/selectors.mjs"
import { unofferedAnswers } from "./lib/workspace.mjs"

const steps = ["permission", "allow", "fail", "cancel"]

const meta = {
  name: "scripted-scenarios",
  summary:
    "a scripted agent through the gateway and the window: permission, failure, cancel",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: {
    only: { type: "string" },
    agent: { type: "string", default: "claude" },
    evidence: { type: "string" },
  },
  help: `
Usage: node verification/desktop/scripts/scripted-scenarios.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA).
No harness and no sign-in: the gateway runs scripted-agent.mjs --scenario
scenarios/window.json. It starts its own gateway. --url is not used. --mode
prod previews a production build; the default is a dev server.

Options:
  --agent claude|codex  the harness profile the scripted agent speaks
                        (default: claude)
  --evidence <dir>      write acp.jsonl, mcp.jsonl, gateway.log and views/

Steps, per engine and layout, in order on one page (--only <names> to pick):
  permission  the turn has asked, and the open conversation shows an agent
          approval card whose reason is the scenario's title, and the text
          the scenario streamed before it asked
  allow   the review's allow option runs the scenario's tool and the rest of that branch:
          the window shows the scenario's "allowed" text and a step titled
          as the review was, and the gateway's turn is completed
  fail    the next turn fails after speaking: the window keeps that text,
          and the gateway's turn is failed
  cancel  a new conversation, because the failed prompt retired the provider
          session: the turn speaks and waits; closing the conversation ends it
          cancelled, and the window still shows the text from before the wait

The strings come from the scenario file. A turn the gateway does not reach
is "could not run".`,
}

const fold = (text) =>
  String(text ?? "")
    .replace(/\s+/g, " ")
    .trim()

/** The turn's text parts, joined, as the window joins one message's chunks. */
const replyText = (turn) =>
  fold(
    (turn?.parts ?? [])
      .filter((part) => part.kind === "text" || part.kind === "local_notice")
      .map((part) => part.text ?? "")
      .join(""),
  )

async function pageText(page) {
  return fold(
    await page
      .locator(css.workspace)
      .innerText()
      .catch(() => ""),
  )
}

/** Reads until `ready(view, turn)` or `seconds` pass. The last read, either way. */
async function untilTurn(client, conversationId, index, ready, seconds) {
  const end = Date.now() + seconds * 1000
  let view
  let turn
  do {
    view = await client.conversation.read(conversationId)
    turn = view.messages[index]
    if (turn && ready(view, turn)) return { view, turn }
    await sleep(250)
  } while (Date.now() < end)
  return { view, turn }
}

async function shot(page, options, engine, layout, name) {
  if (!options.shots) return
  mkdirSync(options.shots, { recursive: true })
  await page
    .screenshot({
      path: join(options.shots, `scripted-scenarios-${engine}-${layout}-${name}.png`),
    })
    .catch((error) => log(`screenshot ${name}: ${error.message}`))
}

/** The scenario, the gateway, and the panel's credential. One stack for every engine. */
async function startStack(options) {
  const script = scenarioScript(
    parseScenario(JSON.parse(readFileSync(WINDOW_SCENARIO, "utf8"))),
  )
  const stack = await startGatewayStack(options, "scripted-scenarios", {
    as: "panel",
    scenario: WINDOW_SCENARIO,
    evidence: options.evidence,
  })
  return panelTarget(stack, () => ({
    script,
    endpoint: stack.gateway.url.replace(/^http/, "ws"),
  }))
}

/**
 * Opens a conversation and sends the permission prompt. Resolves once the
 * gateway is waiting on that review, with the conversation's title.
 */
async function ask(stack, agent) {
  const conversationId = randomUUID()
  const marker = `S${randomUUID().slice(0, 8)}`
  const { view, turn } = await agentTurn(
    stack.client,
    conversationId,
    `${marker} ${stack.script.permission}`,
    {
      agent,
      create: true,
      seconds: 60,
      ready: (current) =>
        current.permissions.some((each) => each.origin?.kind === "harness"),
    },
  )
  const asking = view.permissions.some((each) => each.origin?.kind === "harness")
  if (!asking)
    throw new CannotRun(
      `the permission turn ended ${turn?.status ?? "unlisted"} before it asked`,
    )
  const { conversations } = await stack.client.conversation.list({})
  const title = conversations.find(
    (each) => each.conversationId === conversationId,
  )?.title
  if (!title) throw new CannotRun("the conversation has no title to find it by")
  return { conversationId, title, index: view.messages.length - 1, view }
}

const checks = {
  permission: async (page, stack, run) => {
    await page.locator(css.sessionRow, { hasText: run.title }).first().click()
    const card = page.locator(css.agentApproval)
    const visible = await card
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    const failures = []
    if (!visible) failures.push("no agent approval card within 30 s")
    else {
      const permission = run.view.permissions.find(
        (each) => each.origin?.kind === "harness",
      )
      failures.push(
        ...(await unofferedAnswers(card, permission?.options)).map(
          (line) => `the approval card ${line}`,
        ),
      )
      const reason = fold(
        await card
          .locator(css.approvalReason)
          .innerText()
          .catch(() => ""),
      )
      if (reason !== stack.script.title)
        failures.push(
          `the review asks ${JSON.stringify(reason)}, not ${JSON.stringify(stack.script.title)}`,
        )
    }
    const text = await pageText(page)
    if (!text.includes(stack.script.asking))
      failures.push(`the window does not show ${JSON.stringify(stack.script.asking)}`)
    return { seen: { title: run.title, text }, failures }
  },

  allow: async (page, stack, run) => {
    const card = page.locator(css.agentApproval)
    const failures = []
    const permission = run.view.permissions.find(
      (each) => each.origin?.kind === "harness",
    )
    const label = offeredLabel(permission?.options, "allow")
    failures.push(
      ...(await unofferedAnswers(card, permission?.options)).map(
        (line) => `the approval card ${line}`,
      ),
    )
    const button = label ? card.getByRole("button", { name: label, exact: true }) : null
    const clickable = button
      ? await button
          .waitFor({ timeout: 5_000 })
          .then(() => true)
          .catch(() => false)
      : false
    if (!label) failures.push("the review offers no allow")
    else if (!clickable) failures.push(`no "${label}" button on the agent approval card`)
    else await button.click()
    if (failures.length > 0) return { failures }
    const { view, turn } = await untilTurn(
      stack.client,
      run.conversationId,
      run.index,
      (_view, current) => current.status === "completed" || current.status === "failed",
      60,
    )
    if (turn?.status !== "completed")
      failures.push(`the allowed turn is ${turn?.status ?? "missing"}, not completed`)
    const text = await waitFor(
      async () =>
        (await pageText(page)).includes(stack.script.allowed) ? pageText(page) : null,
      20_000,
    )
    const shown = text ?? (await pageText(page))
    if (!shown.includes(stack.script.allowed))
      failures.push(`the window does not show ${JSON.stringify(stack.script.allowed)}`)
    const labels = await page.locator(css.transcriptStep).allTextContents()
    if (!labels.some((label) => fold(label).includes(stack.script.title)))
      failures.push(`no step titled ${JSON.stringify(stack.script.title)}`)
    if ((await card.count()) !== 0) failures.push("the approval card is still shown")
    return { seen: { status: turn?.status ?? null, labels }, failures, view }
  },

  fail: async (page, stack, run) => {
    const { view, turn } = await agentTurn(
      stack.client,
      run.conversationId,
      stack.script.fail,
      {
        agent: run.agent,
        seconds: 60,
      },
    )
    const failures = []
    if (turn.status !== "failed")
      failures.push(`the failed turn is ${turn.status}, not failed`)
    if (!replyText(turn).includes(stack.script.beforeFailure))
      failures.push(
        `the gateway did not keep ${JSON.stringify(stack.script.beforeFailure)}`,
      )
    const shown = await waitFor(
      async () =>
        (await pageText(page)).includes(stack.script.beforeFailure) ? true : null,
      20_000,
    )
    if (!shown)
      failures.push(
        `the window does not show ${JSON.stringify(stack.script.beforeFailure)}`,
      )
    return { seen: { status: turn.status, reply: replyText(turn) }, failures, view }
  },

  cancel: async (page, stack, run) => {
    // The failed prompt retired the provider session, so this turn is a new
    // conversation. Closing that one is what sends session/cancel.
    const conversationId = randomUUID()
    const marker = `C${randomUUID().slice(0, 8)}`
    const { view, turn } = await agentTurn(
      stack.client,
      conversationId,
      `${marker} ${stack.script.cancel}`,
      {
        agent: run.agent,
        create: true,
        seconds: 45,
        ready: (_current, pending) =>
          pending.status === "running" &&
          replyText(pending).includes(stack.script.beforeCancel),
      },
    )
    const failures = []
    let closed
    if (
      turn?.status !== "running" ||
      !replyText(turn).includes(stack.script.beforeCancel)
    )
      failures.push(
        `the cancel turn is ${turn?.status ?? "missing"} (${JSON.stringify(replyText(turn))}), not waiting after ${JSON.stringify(stack.script.beforeCancel)}`,
      )
    else {
      const { conversations } = await stack.client.conversation.list({})
      const title = conversations.find(
        (each) => each.conversationId === conversationId,
      )?.title
      const row = title ? page.locator(css.sessionRow, { hasText: title }).first() : null
      const listed = row
        ? await row
            .waitFor({ timeout: 20_000 })
            .then(() => true)
            .catch(() => false)
        : false
      if (!listed)
        failures.push(
          `no session row for the cancel turn (${JSON.stringify(title)}) within 20 s`,
        )
      else await row.click()
      const shown = listed
        ? await waitFor(
            async () =>
              (await pageText(page)).includes(stack.script.beforeCancel) ? true : null,
            20_000,
          )
        : null
      if (listed && !shown)
        failures.push(
          `the window does not show ${JSON.stringify(stack.script.beforeCancel)}`,
        )
      try {
        await stack.client.conversation.close(conversationId)
        closed = await stack.client.conversation.read(conversationId)
      } catch (error) {
        failures.push(
          `closing the conversation did not leave a readable turn: ${error.message}`,
        )
      }
      const status = closed?.messages?.at(-1)?.status
      if (closed && status !== "cancelled")
        failures.push(`the closed turn is ${status ?? "missing"}, not cancelled`)
      if (closed && shown && !(await pageText(page)).includes(stack.script.beforeCancel))
        failures.push("the window dropped the text it showed before the cancel")
    }
    return {
      seen: {
        before: turn?.status ?? null,
        after: closed?.messages?.at(-1)?.status ?? null,
      },
      failures,
      view: closed ?? view,
    }
  },
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const only = chosen(options.only, steps, options.list)
    const origin = new URL(stack.url).origin
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        const started = Date.now()
        let run
        try {
          run = { ...(await ask(stack, options.agent)), agent: options.agent }
        } catch (error) {
          rep.add(resultOfThrown({ name: "ask", engine, layout }, error))
          for (const name of only)
            rep.add({
              name,
              engine,
              layout,
              cannotRun: true,
              error: "not run: the permission was not asked",
            })
          continue
        }
        if (options.evidence)
          writeView(
            options.evidence,
            `views/${engine}-${layout}-permission.json`,
            run.view,
          )
        let opened
        try {
          opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            layout,
            initScripts: [
              [gatewayHost, { endpoint: stack.endpoint, credential: stack.credential }],
            ],
          })
        } catch (error) {
          rep.add(resultOfThrown({ name: "open", engine, layout }, error))
          for (const name of only)
            rep.add({
              name,
              engine,
              layout,
              cannotRun: true,
              error: "not run: the page did not open",
            })
          continue
        }
        let stopped = null
        try {
          for (const name of only) {
            if (stopped) {
              rep.add({
                name,
                engine,
                layout,
                cannotRun: true,
                error: `not run: ${stopped}`,
              })
              continue
            }
            const at = Date.now()
            let result
            try {
              result = await checks[name](opened.page, stack, run)
            } catch (error) {
              result = resultOfThrown({}, error)
            }
            if (options.evidence && result.view)
              writeView(
                options.evidence,
                `views/${engine}-${layout}-${name}.json`,
                result.view,
              )
            await shot(opened.page, options, engine, layout, name)
            const entry = rep.add({
              name,
              engine,
              layout,
              ms: Date.now() - at,
              seen: result.seen,
              failures: [
                ...(result.failures ?? []),
                ...(result.cannotRun ? [] : opened.errors.splice(0)),
              ],
              harmless: result.cannotRun ? [] : opened.harmless.splice(0),
              ...(result.error
                ? { error: result.error, cannotRun: Boolean(result.cannotRun) }
                : {}),
            })
            if (!entry.ok)
              stopped = `${name} ${entry.cannotRun ? "could not run" : "did not hold"}`
          }
          const late = opened.errors.splice(0)
          const lateHarmless = opened.harmless.splice(0)
          if (late.length > 0 || lateHarmless.length > 0)
            rep.add({
              name: "console",
              engine,
              layout,
              failures: late,
              harmless: lateHarmless,
            })
        } finally {
          await opened.close()
          log(`${engine} ${layout}: ${Date.now() - started} ms`)
        }
      }
    })
  },
  startStack,
)
