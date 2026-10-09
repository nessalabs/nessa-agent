#!/usr/bin/env node
/**
 * The desktop app's window over a real gateway (#419): it connects as the
 * desktop app does, lists the gateway's conversations, opens one and draws
 * its turn, and draws a turn made elsewhere without a reload, once the
 * gateway holds it (the gateway source's subscriptions). Design rows W1–W5 (#419,
 * comment 5972894657, and the revisions after it).
 *
 * The page runs as the desktop app does: the fake host (`lib/fake-host.mjs`)
 * makes `host.kind` native, so `main.tsx` composes `hostGateway` — the
 * endpoint and credential sources over IPC, `connectDevSession`, the gateway
 * source. The host's two answers are the real gateway's: its endpoint, and the
 * panel's credential (`panelCredential`), never printed.
 *
 * The gateway, the dev server and a client as a surface of its own are
 * `lib/gateway-stack.mjs`'s. The client is signed in as the panel too, as the
 * panel window is in the app, and makes each turn, with the agent's own
 * sign-in on this machine.
 *
 * What it does not show: the native host's composition itself (endpoint
 * discovery, readiness, the credential file read), which the Rust host tests
 * cover (`src-tauri/src/surface_credential.rs`,
 * `gateway_endpoint/entrypoint/command.rs`, `gateway/infrastructure/commands.rs`);
 * WKWebView's IPC and the packaged app's `tauri://localhost` origin, which
 * the live `pnpm app` run does.
 *
 * Every step runs on one page per engine and layout, in order; a step that
 * fails, or could not run, stops those after it on that page, which are
 * reported as not run, and a page that does not open — whether it could not
 * run or failed — is reported so, with each of its steps as not run. The next
 * layout and engine still run.
 */
import { randomUUID } from "node:crypto"
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { SERVER, toolPrompt } from "../../../scripts/mcp-test-server/local-gateway.mjs"
import {
  TEXT_REPLY_APP_PROMPT,
  TEXT_REPLY_SCENARIO,
} from "../../../scripts/mcp-test-server/scenarios.mjs"
import { appFrame, oneMount } from "./lib/apps.mjs"
import { noteLiveMountResourceAbort, openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, chosen, log, resultOfThrown } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  agentTurn,
  panelCredential,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import {
  admitOnce,
  callKey,
  lastTurn,
  permissionKey,
  setupOutcome,
} from "./lib/gateway-view.mjs"
import { main } from "./lib/run.mjs"
import { writeView } from "./lib/scripted-evidence.mjs"
import { css, selectorFor } from "./lib/selectors.mjs"
import { inside, settled } from "./lib/workspace.mjs"

const APP_TOOL = "review_rows"
const steps = ["handshake", "lists", "opens", "live", "apps"]

const meta = {
  name: "gateway-window",
  summary:
    "the desktop app's window over a real gateway: its handshake, a conversation, a turn made elsewhere, the server's MCP App",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: {
    only: { type: "string" },
    agent: { type: "string", default: "claude" },
    scripted: { type: "boolean", default: false },
    evidence: { type: "string" },
  },
  help: `
Usage: node verification/desktop/scripts/gateway-window.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA),
the agent's harness installed (crates/nessa-sdk/harnesses/<agent>-acp, or
MCP_LIVE_HARNESSES), and the agent signed in on this machine — or, with
--scripted, neither. It starts its own gateway. --url is not used. --mode prod
previews a production build; the default is a dev server.

Options:
  --agent claude|codex  the agent the gateway runs (default: claude)
  --scripted            run the text-reply scenario (scenarios/text-reply.json)
                        as that agent: no model, no sign-in. A prompt is
                        answered "Ready.", except "show the server's app",
                        which calls review_rows.
  --evidence <dir>      with --scripted, write acp.jsonl, mcp.jsonl, gateway.log
                        and view.json there

Steps, per engine and layout, in order on one page (--only <names> to pick):
  handshake  on the window's own socket, to the host's endpoint, it
          authenticated as client nessa-panel, named surface kind desktop,
          and the gateway answered ok with principal surface:nessa-panel:
          the panel's credential (W4′)
  lists   connected over the host's endpoint and the panel's credential, the
          window lists the gateway's conversation by its title: no failure
          status, no sample (W1)
  opens   the conversation open, its transcript draws the person's message,
          then the agent's reply, exactly as the gateway holds them (W2)
  live    a turn sent from another surface, once the gateway holds it, is
          drawn in the open transcript, in order, the page not reloaded (W3)
  apps    the test server's review_rows app is drawn inline in the main
          window, one frame, live, its document the server's (#574). Dev
          server only: --mode prod leaves this step out unless --only names
          it, and then it could not run. With --scripted the prompt contains
          "show the server's app", which is the text-reply scenario's turn
          that calls review_rows. Each page gets its own conversation, so a
          second engine is not a second call in the first.

After the steps, once handshake has run:
  handshakes  every handshake the window made — its first and each reconnect
          after it — is the handshake step's (W4′)

Every step also fails on a console error, page error or failed request, and
any that arrives after the last step is reported as "console" (W5). A turn
whose reply is not a non-empty text-only one (no tool, no notice, plain
text) is "could not run": the check cannot say how the window draws it.`,
}

/**
 * The agent is to answer with `marker` alone. It comes first, so the title
 * the gateway makes from the message is this conversation's alone.
 */
const prompt = (marker) =>
  `${marker}: reply with exactly that word and nothing else, using no tools.`

/**
 * Sends `text` and waits for its turn to end (`agentTurn`); the view, once it
 * has. A turn that ends otherwise than completed puts the gateway's last
 * output on stderr, and is "could not run"; so is a reply the window does not
 * draw as its text alone, or an empty one (`lastTurn`), which the steps
 * cannot compare the window with.
 */
async function turn({ client, conversationId, gateway }, text, agent, create = false) {
  const { view, turn } = await agentTurn(client, conversationId, text, {
    agent,
    create,
  })
  if (turn.status !== "completed") {
    log(gateway.log().slice(-4000))
    throw new CannotRun(
      `${agent}'s turn ended ${turn.status}${turn.error ? ` (${JSON.stringify(turn.error)})` : ""}`,
    )
  }
  lastTurn(view)
  return view
}

/** The gateway, the dev server, and a conversation with one finished turn. */
async function startStack(options) {
  // The conversation is the panel's, as one the panel window began is: the
  // window lists what its credential's principal holds. --scripted answers
  // with the text-reply scenario, which the steps compare as any text reply.
  const stack = await startGatewayStack(options, "gateway-window", {
    as: "panel",
    ...(options.scripted
      ? { scenario: TEXT_REPLY_SCENARIO, evidence: options.evidence }
      : {}),
  })
  try {
    const started = Date.now()
    const conversationId = randomUUID()
    const marker = `W${randomUUID().slice(0, 8)}`
    const view = await turn(
      { ...stack, conversationId },
      prompt(marker),
      options.agent,
      true,
    )
    if (options.evidence) writeView(options.evidence, "view.json", view)
    stack.timings.agentTurnMs = Date.now() - started
    const { conversations } = await stack.client.conversation.list({})
    const title = conversations.find(
      (each) => each.conversationId === conversationId,
    )?.title
    if (!title) throw new CannotRun("the conversation has no title to find it by")
    log(`conversation ready in ${stack.timings.agentTurnMs} ms`)
    return {
      ...stack,
      endpoint: stack.gateway.url.replace(/^http/, "ws"),
      conversationId,
      title,
      credential: panelCredential(stack.gateway),
    }
  } catch (error) {
    await stack.close()
    throw error
  }
}

/**
 * Watches `page`'s sockets for the product handshake, into `seen`: each
 * `session.authenticate` the page sent, with the socket's URL, the client
 * id it named, the surface kind and instance, and the gateway's answer.
 * Only those fields are kept: the request carries the credential, and no
 * frame is.
 */
function watchHandshakes(page, seen) {
  page.on("websocket", (socket) => {
    const asked = new Map()
    const parse = (payload) => {
      try {
        return JSON.parse(String(payload))
      } catch {
        return null
      }
    }
    socket.on("framesent", ({ payload }) => {
      const frame = parse(payload)
      if (frame?.method !== "session.authenticate") return
      const entry = {
        socket: socket.url(),
        client: frame.params?.client?.id ?? null,
        surface: {
          kind: frame.params?.surface?.kind ?? null,
          instance: frame.params?.surface?.instance ?? null,
        },
        answer: null,
      }
      asked.set(frame.id, entry)
      seen.push(entry)
    })
    socket.on("framereceived", ({ payload }) => {
      const frame = parse(payload)
      const entry = frame?.type === "res" ? asked.get(frame.id) : undefined
      if (!entry) return
      entry.answer = frame.ok
        ? { ok: true, principal: frame.payload?.principalId ?? null }
        : { ok: false, code: frame.error?.code ?? null }
    })
  })
}

/**
 * The open transcript's messages: role, text with its whitespace folded, and
 * whether inside the chat area — its sides and top; a message may run on
 * below it, into the scroll.
 */
async function transcript(page) {
  const { area, messages } = await page.evaluate(
    ([message, chat]) => {
      const rect = (element) => {
        const r = element.getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const area = document.querySelector(chat)
      return {
        area: area ? rect(area) : null,
        messages: [...document.querySelectorAll(message)].map((element) => ({
          role: element.getAttribute("data-role"),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim(),
          rect: rect(element),
        })),
      }
    },
    [css.message, css.chatArea],
  )
  return messages.map(({ role, text, rect }) => ({
    role,
    text,
    inside: area !== null && inside(rect, area, ["left", "top", "right"]),
  }))
}

/**
 * Failures for `turn` (`{ user, reply }`, as the gateway holds it) as the last
 * two messages of `messages`: the person's message as sent, then the reply.
 */
function drawn(messages, turn) {
  const failures = []
  const [user, agent] = messages.slice(-2)
  if (user?.role !== "user" || user.text !== turn.user)
    failures.push(
      `the person's message is ${JSON.stringify(user ?? null)}, not ${JSON.stringify(turn.user)}`,
    )
  // `turn` is a text-only reply (`lastTurn`), which the window draws as its
  // text alone.
  if (agent?.role !== "agent" || agent.text !== turn.reply)
    failures.push(
      `the reply is ${JSON.stringify(agent ?? null)}, not ${JSON.stringify(turn.reply)}`,
    )
  for (const each of [user, agent])
    if (each && !each.inside)
      failures.push(`the ${each.role}'s message is outside the chat area`)
  return failures
}

/** The handshakes as kept: socket, client id, surface kind and instance, answer. */
const shown = (handshakes) =>
  handshakes.map(({ socket, client, surface, answer }) => ({
    socket,
    client,
    surface,
    answer,
  }))

/**
 * Failures for the window's handshakes `seen` (W4′): at least one, and
 * each on a socket to the host's endpoint — not the dev server's `/browser`
 * proxy to the same gateway — authenticated as client nessa-panel, naming
 * surface kind desktop with an instance, and answered ok with principal
 * surface:nessa-panel.
 */
function handshakeFailures(seen, stack) {
  const failures = []
  if (seen.length === 0) failures.push("the window's socket carried no handshake")
  const host = new URL(stack.endpoint)
  for (const each of seen) {
    const url = new URL(each.socket)
    if (url.protocol !== host.protocol || url.host !== host.host)
      failures.push(
        `the window's socket is ${each.socket}, not the host's ${stack.endpoint}`,
      )
    if (each.client !== "nessa-panel")
      failures.push(
        `the window authenticated as ${JSON.stringify(each.client)}, not nessa-panel`,
      )
    if (each.surface?.kind !== "desktop")
      failures.push(
        `the window's surface is ${JSON.stringify(each.surface ?? null)}, not desktop`,
      )
    if (typeof each.surface?.instance !== "string" || each.surface.instance.length === 0)
      failures.push(
        `the window's surface instance is ${JSON.stringify(each.surface?.instance ?? null)}`,
      )
    if (!each.answer?.ok || each.answer.principal !== "surface:nessa-panel")
      failures.push(
        `the gateway answered ${JSON.stringify(each.answer)}, not surface:nessa-panel`,
      )
  }
  return failures
}

const checks = {
  lists: async (page, stack) => {
    const failures = []
    const row = page.locator(css.sessionRow, { hasText: stack.title }).first()
    const listed = await row
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    const seen = await page.evaluate(
      ([empty, sample]) => ({
        status: document.querySelector(empty)?.textContent ?? null,
        sample: document.querySelectorAll(sample).length,
        asked: { ...window.__fakeHostAsked },
      }),
      [css.workspaceEmpty, css.sampleAccessory],
    )
    if (!listed)
      failures.push(`no session row "${stack.title}" in the window within 30 s`)
    if (seen.status !== null)
      failures.push(`the window says ${JSON.stringify(seen.status)}`)
    if (seen.sample !== 0) failures.push("the sample plugin is drawn")
    // Counted, and at least once: a count that is missing fails too.
    if (!(seen.asked.load_gateway_endpoint >= 1))
      failures.push("the host's endpoint was never asked")
    if (!(seen.asked.load_surface_credential >= 1))
      failures.push("the host's credential was never asked")
    return { seen: { listed, ...seen }, failures }
  },

  handshake: async (page, stack, options, handshakes) => {
    // The window's first handshake, answered.
    await waitFor(() => handshakes.some((each) => each.answer !== null), 30_000)
    const seen = shown(handshakes)
    return { seen, failures: handshakeFailures(seen, stack) }
  },

  opens: async (page, stack) => {
    await page.locator(css.sessionRow, { hasText: stack.title }).first().click()
    // Waiting on the product: a transcript that never draws has failed.
    const shown = await page
      .locator(css.message)
      .first()
      .waitFor({ timeout: 20_000 })
      .then(() => true)
      .catch(() => false)
    if (!shown)
      return { seen: {}, failures: ["the open conversation drew no message within 20 s"] }
    await settled(page)
    const messages = await transcript(page)
    // The last turn as the gateway holds it now: an engine before this one
    // added its own live turn.
    const last = lastTurn(await stack.client.conversation.read(stack.conversationId))
    return { seen: { messages }, failures: drawn(messages, last) }
  },

  live: async (page, stack, options) => {
    const before = (await transcript(page)).length
    // A mark on this document: a reload would take it away.
    const mark = randomUUID()
    await page.evaluate((mark) => (window.__gatewayWindowMark = mark), mark)
    const marker = `W${randomUUID().slice(0, 8)}`
    const view = await turn(stack, prompt(marker), options.agent)
    const second = lastTurn(view)
    // The turn has ended at the gateway; the window has its events by now or soon.
    const messages = await waitFor(async () => {
      const now = await transcript(page)
      return drawn(now, second).length === 0 ? now : null
    }, 15_000)
    const now = messages ?? (await transcript(page))
    const failures = drawn(now, second)
    if (now.length !== before + 2)
      failures.push(`${now.length} messages after the turn, not ${before + 2}`)
    const kept = await page.evaluate(() => window.__gatewayWindowMark)
    if (kept !== mark) failures.push("the page was reloaded during the turn")
    return { seen: { before, messages: now, reloaded: kept !== mark }, failures }
  },

  apps: async (page, stack, options) => {
    if (options.mode === "prod") throw new CannotRun("not run: dev server only")
    const failures = []
    const conversationId = randomUUID()
    const marker = `A${randomUUID().slice(0, 8)}`
    const prompt = options.scripted
      ? `${marker}: ${TEXT_REPLY_APP_PROMPT}`
      : toolPrompt([{ name: APP_TOOL }])
    let admitted = null
    const answered = new Set()
    const { view, turn: ended } = await agentTurn(stack.client, conversationId, prompt, {
      agent: options.agent,
      create: true,
      onView: async (current) => {
        for (;;) {
          const { allow, extra } = admitOnce(
            current,
            admitted,
            answered,
            SERVER,
            APP_TOOL,
          )
          if (extra)
            throw new CannotRun(
              `${options.agent} called ${APP_TOOL} more than once (${admitted}, ${extra})`,
            )
          if (!allow) break
          admitted = allow.call
          answered.add(permissionKey(allow.permission))
          await stack.client.conversation.answer(
            conversationId,
            allow.permission.executionId,
            allow.permission.permissionId,
            allow.option.id,
          )
        }
      },
    })
    if (ended.status !== "completed")
      throw new CannotRun(`${options.agent}'s app turn ended ${ended.status}`)
    const outcome = setupOutcome(view, SERVER, APP_TOOL)
    if (outcome.kind !== "ready")
      throw new CannotRun(
        `${APP_TOOL} is ${outcome.kind}${
          outcome.call ? ` (${outcome.call.status}, ${callKey(outcome.call)})` : ""
        }`,
      )
    const { conversations } = await stack.client.conversation.list({})
    const title = conversations.find(
      (each) => each.conversationId === conversationId,
    )?.title
    if (!title) throw new CannotRun("the app conversation has no title to find it by")
    const row = page.locator(css.sessionRow, { hasText: title }).first()
    const listed = await row
      .waitFor({ timeout: 30_000 })
      .then(() => true)
      .catch(() => false)
    if (!listed)
      return {
        seen: { title },
        failures: [`no session row "${title}" for the app turn`],
      }
    await row.click()
    await settled(page)
    const drawn = await page
      .waitForSelector(selectorFor.appFrameIn("inline"), {
        timeout: 30_000,
        state: "attached",
      })
      .then(() => true)
      .catch(() => false)
    const frames = drawn ? (await page.$$(selectorFor.appFrameIn("inline"))).length : 0
    const once = oneMount(frames)
    if (once) failures.push(once)
    if (!drawn) return { seen: { frames }, failures }
    let framed
    try {
      framed = await appFrame(page, "inline", 30_000)
    } catch (error) {
      if (
        error instanceof CannotRun &&
        error.message === "no app document in the inline frame"
      ) {
        failures.push(error.message)
        return { seen: { frames }, failures }
      }
      throw error
    }
    const { app } = framed
    await app
      .waitForSelector(selectorFor.reviewState("live"), { timeout: 20_000 })
      .catch(() => {})
    const seen = await app.evaluate(() => ({
      state: document.body.getAttribute("data-review-state"),
      heading: document.querySelector("h1")?.textContent ?? null,
    }))
    seen.frames = frames
    if (seen.state !== "live") failures.push(`the app is ${seen.state}, not live`)
    if (seen.heading !== "Review rows (nessa-test)")
      failures.push(`the app's document is not the server's: ${seen.heading}`)
    return { seen, failures }
  },
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const only = chosen(options.only, steps, options.list).filter(
      (name) => name !== "apps" || options.mode !== "prod" || Boolean(options.only),
    )
    rep.add({
      name: "setup",
      seen: { title: stack.title },
      timings: stack.timings,
      failures: [],
    })
    const origin = new URL(stack.url).origin
    const endpoint = stack.endpoint
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        const started = Date.now()
        // Every handshake the window's sockets make, from its first.
        const handshakes = []
        let opened
        try {
          opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            layout,
            lines: { engine, layout },
            initScripts: [[gatewayHost, { endpoint, credential: stack.credential }]],
            // The window drawn; what it lists, or says, is the steps' to judge.
            beforeLoad: (context) =>
              context.on("page", (page) => watchHandshakes(page, handshakes)),
          })
        } catch (error) {
          // Could not run, or failed: reported either way, and the next
          // layout and engine still run.
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
        const ran = new Set()
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
            ran.add(name)
            let result
            try {
              result = await checks[name](opened.page, stack, options, handshakes)
            } catch (error) {
              // Could not run (an agent turn the steps cannot read, say), or
              // failed: either way the steps after it are not run.
              result = resultOfThrown({}, error)
            }
            if (
              options.shots &&
              !result.cannotRun &&
              (result.failures ?? []).length === 0
            ) {
              mkdirSync(options.shots, { recursive: true })
              const surface =
                name === "apps"
                  ? opened.page.locator(selectorFor.appFrameIn("inline")).first()
                  : opened.page
              await surface
                .screenshot({
                  path: join(
                    options.shots,
                    `gateway-window-${engine}-${layout}-${name}.png`,
                  ),
                })
                .catch((error) => log(`screenshot ${name}: ${error.message}`))
            }
            await opened.settleRequests()
            // Live mount proof covers the exact resource abort even if the
            // browser delivers its line after this step, before close (#647).
            if (name === "apps") {
              noteLiveMountResourceAbort(opened, result.seen?.state)
            }
            const entry = rep.add({
              name,
              engine,
              layout,
              ms: Date.now() - at,
              ...result,
            })
            if (!entry.ok)
              stopped = `${name} ${entry.cannotRun ? "could not run" : "did not hold"}`
          }
          // Every handshake the window made, its reconnects' too, not only
          // the first the handshake step saw (W4′).
          if (ran.has("handshake")) {
            const seen = shown(handshakes)
            rep.add({
              name: "handshakes",
              engine,
              layout,
              seen,
              failures: handshakeFailures(seen, stack),
            })
          }
        } finally {
          await opened.close()
          log(`${engine} ${layout}: ${Date.now() - started} ms`)
        }
      }
    })
  },
  startStack,
)
