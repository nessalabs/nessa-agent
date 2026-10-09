#!/usr/bin/env node
/**
 * A saved part this build could not read is one muted row, and the messages
 * around it still show. The gateway lists one conversation and answers
 * `conversation.read` with two completed turns and one `unreadable` part
 * between them.
 *
 * The page runs as the desktop app does: the fake host (`lib/fake-host.mjs`)
 * makes `host.kind` native, so `main.tsx` composes `hostGateway`. The socket
 * is Playwright's (`routeWebSocket`).
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"
import { inside } from "./lib/workspace.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
const conversationId = "11111111-1111-4111-8111-111111111111"
const title = "Gapped conversation"
const sentence = "Couldn't read this part of the conversation"

const grant = (action) => ({
  action,
  resource: { organizationId: "personal", id: "gateway" },
})

const sessionReady = {
  version: 1,
  gatewayId: "gateway",
  principalId: "owner",
  organizationId: "personal",
  membershipId: "membership",
  credentialId: "owner-credential",
  audienceId: "gateway",
  expiresAt: 2_000_000_000,
  grants: [grant("conversation.read")],
  methods: ["server.health", "conversation.list", "conversation.read"],
}

const listed = {
  conversationId,
  title,
  preview: "Kept after",
  createdAtMs: 1_700_000_000_000,
  updatedAtMs: 1_700_000_100_000,
  running: true,
  archived: false,
}

const message = (executionId, userText, text) => ({
  executionId,
  userText,
  attachments: [],
  files: [],
  status: "completed",
  parts: [{ offset: 0, kind: "text", text, toolId: "", noticeId: "" }],
})

const read = {
  conversationId,
  revision: "1",
  title,
  messages: [
    message("before", "Kept before", "Answer before"),
    message("after", "Kept after", "Answer after"),
  ],
  pending: [],
  permissions: [],
  questions: [],
  tools: [],
  capabilities: {
    queue: true,
    steer: false,
    resume: true,
    permissions: true,
    imageInput: false,
    agentFeatures: {
      permissionDenial: "unknown",
      nativeHookSuppression: "unknown",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported_not_implemented",
      policyCloseSession: "unsupported_not_implemented",
      incomingElicitation: "unsupported",
    },
  },
  lifecycle: { phase: "attached" },
  truncated: false,
  queueComplete: true,
  transcriptState: "complete",
  approvalMode: "ask",
  approvalModes: [
    { id: "ask", name: "Ask", description: "Ask before tools." },
  ],
  unreadable: [
    {
      session: conversationId,
      position: 4,
      reason: "another_version",
      found: 2,
      afterMessage: 1,
    },
  ],
}

/**
 * A product socket that lists one conversation and reads it with a gap
 * between two turns. `seen` names any method this check does not answer.
 */
function gappedGateway(seen) {
  function route(socket) {
    const nonce = `gap-${Math.random().toString(16).slice(2)}`
    socket.send(
      JSON.stringify({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload: { minVersion: 1, maxVersion: 1, nonce, expiresAt: 2_000_000_000 },
      }),
    )
    socket.onMessage((raw) => {
      let frame
      try {
        frame = JSON.parse(String(raw))
      } catch {
        return
      }
      if (!frame || typeof frame.id !== "string") return
      const send = (result) =>
        socket.send(JSON.stringify({ type: "res", id: frame.id, ...result }))
      if (frame.method === "session.authenticate") {
        const matches = frame.params?.nonce === nonce
        send(
          matches
            ? { ok: true, payload: sessionReady }
            : { ok: false, error: { code: "unauthorized", message: "nonce" } },
        )
        return
      }
      if (frame.method === "server.health") {
        send({ ok: true, payload: { ok: true, runtimeStatus: "ready", uptimeMs: 1 } })
        return
      }
      if (frame.method === "conversation.list") {
        const archived = frame.params?.archived === true
        send({
          ok: true,
          payload: {
            conversations: archived ? [] : [listed],
            complete: true,
          },
        })
        return
      }
      if (frame.method === "conversation.read") {
        seen.reads += 1
        if (frame.params?.conversationId !== conversationId)
          seen.unexpected.push(`conversation.read ${String(frame.params?.conversationId)}`)
        send({ ok: true, payload: read })
        return
      }
      seen.unexpected.push(frame.method ?? "frame")
      send({ ok: false, error: { code: "invalid_request", message: "not this check" } })
    })
  }
  return route
}

/** The drawn order of messages and the unreadable row, plus where the row sits. */
async function drawn(page) {
  return page.evaluate(
    ([part, message, chat]) => {
      const rect = (element) => {
        const r = element.getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const row = document.querySelector(part)
      const area = document.querySelector(chat)
      const order = [...document.querySelectorAll(`${message}, ${part}`)].map((element) => ({
        text: element.textContent,
        gap: element.matches(part),
        button: element.querySelector("button") !== null,
        session: element.getAttribute("data-unreadable-session"),
        position: element.getAttribute("data-unreadable-position"),
        reason: element.getAttribute("data-unreadable-reason"),
        found: element.getAttribute("data-unreadable-found"),
      }))
      return {
        order,
        part: row ? rect(row) : null,
        chat: area ? rect(area) : null,
      }
    },
    [css.unreadablePart, css.message, css.chatArea],
  )
}

await main(
  {
    name: "unreadable-part",
    summary: "a part this build could not read is one muted row, and the messages around it still show",
    defaults: { engine: "chromium,webkit" },
    help: `
The gateway lists one conversation and reads it with two turns and one
unreadable part between them. The row sits between those turns, names the
session, position, and reason, and has no button.`,
  },
  async ({ options, rep, url }) => {
    const origin = new URL(url).origin
    await withEngines(options, rep, async (engine, browser) => {
      await attempt(rep, { name: "between", engine, width: 1440 }, async () => {
        const seen = { reads: 0, unexpected: [] }
        let opened
        try {
          opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            width: 1440,
            height: 900,
            initScripts: [[gatewayHost, { endpoint: fakeGateway, credential: "fixture-only" }]],
            readySelector: css.unreadablePart,
            beforeLoad: async (context) => {
              await context.routeWebSocket(`${fakeGateway}/**`, gappedGateway(seen))
            },
          })
          // The transcript pins to the latest message by writing scrollTop
          // inside its existing ResizeObserver. Once the turns overflow,
          // WebKit reports the browser's own loop limit. Chromium does not.
          // The row does not install that observer.
          opened.noteHarmless(
            /^pageerror: ResizeObserver loop completed with undelivered notifications\.?$/,
          )
          const { page } = opened
          await need(page, css.unreadablePart, "the unreadable row")
          const view = await drawn(page)
          const failures = []
          const texts = view.order.map((row) => row.text)
          const gapAt = view.order.findIndex((row) => row.gap)
          const before = texts.findIndex((text) => text?.includes("Kept before"))
          const answer = texts.findIndex((text) => text?.includes("Answer before"))
          const after = texts.findIndex((text) => text?.includes("Kept after"))
          const later = texts.findIndex((text) => text?.includes("Answer after"))
          if (before < 0 || answer < 0 || after < 0 || later < 0)
            failures.push(`transcript texts are ${JSON.stringify(texts)}`)
          if (!(before < gapAt && answer < gapAt && gapAt < after && gapAt < later))
            failures.push(`the row is not between the turns: ${JSON.stringify(texts)}`)
          const gap = view.order[gapAt]
          if (!gap) failures.push("no unreadable row")
          else {
            if (gap.text !== sentence)
              failures.push(`the row says ${JSON.stringify(gap.text)}`)
            if (gap.button) failures.push("the row has a button")
            if (gap.session !== conversationId)
              failures.push(`the row session is ${JSON.stringify(gap.session)}`)
            if (gap.position !== "4") failures.push(`the row position is ${gap.position}`)
            if (gap.reason !== "another_version")
              failures.push(`the row reason is ${JSON.stringify(gap.reason)}`)
            if (gap.found !== "2") failures.push(`the row found is ${JSON.stringify(gap.found)}`)
          }
          if (!view.part || !view.chat) failures.push("no unreadable row in the chat area")
          else if (!inside(view.part, view.chat))
            failures.push("the unreadable row is outside the chat area")
          if (seen.reads < 1) failures.push("the conversation was never read")
          if (seen.unexpected.length > 0)
            failures.push(`unexpected calls: ${seen.unexpected.join(", ")}`)
          if (options.shots && failures.length === 0) {
            mkdirSync(options.shots, { recursive: true })
            await page.locator(css.chatArea).screenshot({
              path: join(options.shots, `transcript-row-${engine}.png`),
            })
          }
          return { failures, measured: { reads: seen.reads, texts } }
        } finally {
          await opened?.close().catch(() => {})
        }
      })
    })
  },
)
