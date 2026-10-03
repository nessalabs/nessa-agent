#!/usr/bin/env node
/**
 * The desktop app's window over a real gateway (#419): it connects as the
 * desktop app does, lists the gateway's conversations, opens one and draws
 * its turn, and draws a turn made elsewhere without a reload, once the
 * gateway holds it (the gateway source's poller). Design rows W1–W5 (#419,
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
 * Every step runs on one page per engine, in order; a step that fails stops
 * those after it, which are reported as not run.
 */
import { randomUUID } from "node:crypto"
import { setTimeout as sleep } from "node:timers/promises"

import { turnEnded } from "../../../scripts/mcp-test-server/evidence.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, chosen, log } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { panelCredential, startGatewayStack, waitFor } from "./lib/gateway-stack.mjs"
import { lastTurn } from "./lib/gateway-view.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"
import { settled } from "./lib/workspace.mjs"

const steps = ["handshake", "lists", "opens", "live"]

const meta = {
  name: "gateway-window",
  summary:
    "the desktop app's window over a real gateway: its handshake, a conversation, a turn made elsewhere",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { only: { type: "string" }, agent: { type: "string", default: "claude" } },
  help: `
Usage: node verification/desktop/scripts/gateway-window.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA),
the agent's harness installed (crates/nessa-sdk/harnesses/<agent>-acp, or
MCP_LIVE_HARNESSES), and the agent signed in on this machine. It starts its own
gateway and dev server; --url and --mode are not used.

Options:
  --agent claude|codex  the agent the gateway runs (default: claude)

Steps, per engine and layout, in order on one page (--only <names> to pick):
  handshake  on the window's own socket, to the host's endpoint, it
          authenticated as client nessa-panel and the gateway answered ok with
          principal surface:nessa-panel: the panel's credential (W4)
  lists   connected over the host's endpoint and the panel's credential, the
          window lists the gateway's conversation by its title: no failure
          status, no sample (W1)
  opens   the conversation open, its transcript draws the person's message,
          then the agent's reply, exactly as the gateway holds them (W2)
  live    a turn sent from another surface, once the gateway holds it, is
          drawn in the open transcript, in order, the page not reloaded (W3)

Every step also fails on a console error, page error or failed request, and
any that arrives after the last step is reported as "console" (W5).`,
}

/**
 * The agent is to answer with `marker` alone. It comes first, so the title
 * the gateway makes from the message is this conversation's alone.
 */
const prompt = (marker) =>
  `${marker}: reply with exactly that word and nothing else, using no tools.`

/**
 * Sends `text` and waits for its turn to end; the view, once it has. A turn
 * that ends otherwise than completed puts the gateway's last output on stderr,
 * and is "could not run".
 */
async function turn({ client, conversationId, gateway }, text, agent) {
  const before = (await client.conversation.read(conversationId)).messages.length
  try {
    await client.conversation.send(conversationId, text)
  } catch (error) {
    throw new CannotRun(`the gateway refused the ${agent} turn: ${error.message}`)
  }
  let last
  for (let i = 0; i < 180; i += 1) {
    await sleep(1000)
    const view = await client.conversation.read(conversationId)
    // The view's messages are its turns; this one is the one after `before`.
    last = view.messages[before]
    if (last && turnEnded(last.status)) {
      if (last.status !== "completed") {
        log(gateway.log().slice(-4000))
        throw new CannotRun(
          `${agent}'s turn ended ${last.status}${last.error ? ` (${JSON.stringify(last.error)})` : ""}`,
        )
      }
      return view
    }
  }
  throw new CannotRun(
    `${agent}'s turn did not end within 180 s; it was last ${last?.status ?? "not listed"}`,
  )
}

/** The gateway, the dev server, and a conversation with one finished turn. */
async function startStack(options) {
  // The conversation is the panel's, as one the panel window began is: the
  // window lists what its credential's principal holds.
  const stack = await startGatewayStack(options, "gateway-window", { as: "panel" })
  try {
    const started = Date.now()
    const conversationId = randomUUID()
    try {
      await stack.client.conversation.create({ conversationId, agent: options.agent })
    } catch (error) {
      throw new CannotRun(
        `the gateway refused the ${options.agent} conversation (is ${options.agent} signed in on this machine?): ${error.message}`,
      )
    }
    const marker = `W${randomUUID().slice(0, 8)}`
    await turn({ ...stack, conversationId }, prompt(marker), options.agent)
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
 * `session.authenticate` the page sent, with the socket's URL and the client
 * id it named, and the gateway's answer to it. Only those fields are kept:
 * the request carries the credential, and no frame is.
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

/** The open transcript's messages: role, text with its whitespace folded, and whether inside the chat area. */
const transcript = (page) =>
  page.evaluate(
    ([message, chat]) => {
      const area = document.querySelector(chat)?.getBoundingClientRect()
      return [...document.querySelectorAll(message)].map((element) => {
        const r = element.getBoundingClientRect()
        return {
          role: element.getAttribute("data-role"),
          text: (element.textContent ?? "").replace(/\s+/g, " ").trim(),
          inside: area
            ? r.left >= area.left - 0.5 &&
              r.right <= area.right + 0.5 &&
              r.top >= area.top - 0.5
            : false,
        }
      })
    },
    [css.message, css.chatArea],
  )

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
  // A text-only reply is drawn as its text alone (`message.tsx`); the prompt
  // asks for no tools, so nothing else may be in it.
  if (!turn.reply || agent?.role !== "agent" || agent.text !== turn.reply)
    failures.push(
      `the reply is ${JSON.stringify(agent ?? null)}, not ${JSON.stringify(turn.reply)}`,
    )
  for (const each of [user, agent])
    if (each && !each.inside)
      failures.push(`the ${each.role}'s message is outside the chat area`)
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
    if (seen.asked.load_gateway_endpoint < 1)
      failures.push("the host's endpoint was never asked")
    if (seen.asked.load_surface_credential < 1)
      failures.push("the host's credential was never asked")
    return { seen: { listed, ...seen }, failures }
  },

  handshake: async (page, stack, options, handshakes) => {
    const failures = []
    // The window's first handshake, answered.
    await waitFor(() => handshakes.some((each) => each.answer !== null), 30_000)
    const seen = handshakes.map(({ socket, client, answer }) => ({
      socket,
      client,
      answer,
    }))
    if (seen.length === 0) failures.push("the window's socket carried no handshake")
    // The host's endpoint, not the dev server's `/browser` proxy to the same gateway.
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
      if (!each.answer?.ok || each.answer.principal !== "surface:nessa-panel")
        failures.push(
          `the gateway answered ${JSON.stringify(each.answer)}, not surface:nessa-panel`,
        )
    }
    return { seen, failures }
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
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const only = chosen(options.only, steps, options.list)
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
        const opened = await openPage(browser, {
          url: `${origin}/desktop.html`,
          layout,
          initScripts: [[gatewayHost, { endpoint, credential: stack.credential }]],
          // The window drawn; what it lists, or says, is the steps' to judge.
          beforeLoad: (context) =>
            context.on("page", (page) => watchHandshakes(page, handshakes)),
        })
        let stopped = null
        try {
          for (const name of only) {
            if (stopped) {
              rep.add({
                name,
                engine,
                layout,
                cannotRun: true,
                error: `not run: ${stopped} failed`,
              })
              continue
            }
            const at = Date.now()
            let result
            try {
              result = await checks[name](opened.page, stack, options, handshakes)
            } catch (error) {
              if (error instanceof CannotRun) throw error
              result = { failures: [], error: error.message.split("\n")[0] }
            }
            const entry = rep.add({
              name,
              engine,
              layout,
              ms: Date.now() - at,
              ...result,
              failures: [...(result.failures ?? []), ...opened.errors.splice(0)],
            })
            if (!entry.ok) stopped = name
          }
          // What the page said after the last step, before it closes.
          const late = opened.errors.splice(0)
          if (late.length > 0)
            rep.add({ name: "console", engine, layout, failures: late })
        } finally {
          await opened.close()
          log(`${engine} ${layout}: ${Date.now() - started} ms`)
        }
      }
    })
  },
  startStack,
)
