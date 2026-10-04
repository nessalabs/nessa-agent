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
 * Every step runs on one page per engine and layout, in order; a step that
 * fails, or could not run, stops those after it on that page, which are
 * reported as not run, and a page that does not open — whether it could not
 * run or failed — is reported so, with each of its steps as not run. The next
 * layout and engine still run.
 */
import { randomUUID } from "node:crypto"

import { openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, chosen, log, resultOfThrown } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import {
  agentTurn,
  panelCredential,
  startGatewayStack,
  waitFor,
} from "./lib/gateway-stack.mjs"
import { lastTurn } from "./lib/gateway-view.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"
import { inside, settled } from "./lib/workspace.mjs"

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
          principal surface:nessa-panel: the panel's credential (W4′)
  lists   connected over the host's endpoint and the panel's credential, the
          window lists the gateway's conversation by its title: no failure
          status, no sample (W1)
  opens   the conversation open, its transcript draws the person's message,
          then the agent's reply, exactly as the gateway holds them (W2)
  live    a turn sent from another surface, once the gateway holds it, is
          drawn in the open transcript, in order, the page not reloaded (W3)

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
  // window lists what its credential's principal holds.
  const stack = await startGatewayStack(options, "gateway-window", { as: "panel" })
  try {
    const started = Date.now()
    const conversationId = randomUUID()
    const marker = `W${randomUUID().slice(0, 8)}`
    await turn({ ...stack, conversationId }, prompt(marker), options.agent, true)
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

/** The handshakes as kept: no more than their socket, client id and answer. */
const shown = (handshakes) =>
  handshakes.map(({ socket, client, answer }) => ({ socket, client, answer }))

/**
 * Failures for the window's handshakes `seen` (W4′): at least one, and
 * each on a socket to the host's endpoint — not the dev server's `/browser`
 * proxy to the same gateway — authenticated as client nessa-panel and
 * answered ok with principal surface:nessa-panel.
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
        let opened
        try {
          opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            layout,
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
            const entry = rep.add({
              name,
              engine,
              layout,
              ms: Date.now() - at,
              ...result,
              // A step that could not run keeps none: they go to "console".
              failures: [
                ...(result.failures ?? []),
                ...(result.cannotRun ? [] : opened.errors.splice(0)),
              ],
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
