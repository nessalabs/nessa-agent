#!/usr/bin/env node
/**
 * A conversation the window listed but could not read (#433). The gateway
 * answers the handshake and lists one running conversation, then refuses
 * `conversation.read` as `temporarily_unavailable`. The open transcript's
 * note, and the Agents peek, say what could not be read — the conversation
 * sentence in `failure-copy.ts`, never the unconfirmed-call sentence.
 *
 * The page runs as the desktop app does: the fake host (`lib/fake-host.mjs`)
 * makes `host.kind` native, so `main.tsx` composes `hostGateway`. The socket
 * is Playwright's (`routeWebSocket`).
 *
 * What it does not show: a request row's unreadable. Needs you is a
 * conversation whose read already asked the person something, so a first
 * read that fails never lands there. `overview.test.tsx` holds that sentence.
 */
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { main } from "./lib/run.mjs"
import { css, keys } from "./lib/selectors.mjs"
import { focusComposer, inside } from "./lib/workspace.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
const conversationId = "11111111-1111-4111-8111-111111111111"
const title = "Unread conversation"
/** `readFailureCopy(_, "conversation")` for `unavailable`. The unit test pins the same words. */
const unread = "Nessa couldn’t read this conversation just now."
/** `failureCopy` for `unavailable`: the sentence a conversation read must not borrow. */
const unconfirmed = "Nessa couldn’t confirm this just now."

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
  preview: "still going",
  createdAtMs: 1_700_000_000_000,
  updatedAtMs: 1_700_000_100_000,
  running: true,
  archived: false,
}

/**
 * A product socket that lists one running conversation and refuses to read
 * it. `seen` counts the reads and names any method this check does not answer,
 * so a new call does not pass by being ignored.
 */
function unreadGateway(seen) {
  function route(socket) {
    const nonce = `unread-${Math.random().toString(16).slice(2)}`
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
            conversations: archived ? [] : [{ ...listed, archived }],
            complete: true,
          },
        })
        return
      }
      if (frame.method === "conversation.read") {
        seen.reads += 1
        if (frame.params?.conversationId !== conversationId)
          seen.unexpected.push(
            `conversation.read ${String(frame.params?.conversationId)}`,
          )
        send({
          ok: false,
          error: { code: "temporarily_unavailable", message: "temporarily_unavailable" },
        })
        return
      }
      seen.unexpected.push(frame.method ?? "frame")
      send({ ok: false, error: { code: "invalid_request", message: "not this check" } })
    })
  }
  return route
}

/** The note's sentence and where it sits, or nulls when it is not drawn. */
async function noteOf(page) {
  return page.evaluate(
    ([note, text, chat]) => {
      const rect = (element) => {
        const r = element.getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const status = document.querySelector(note)
      const area = document.querySelector(chat)
      return {
        text: document.querySelector(text)?.textContent ?? null,
        note: status ? rect(status) : null,
        chat: area ? rect(area) : null,
      }
    },
    [css.transcriptNote, css.transcriptNoteText, css.chatArea],
  )
}

/**
 * Opens Agents the way a person does: the sidebar's entry when it is shown,
 * and otherwise the overview chord. The chord's command key is ⌘ where the
 * browser is a Mac and Control elsewhere (`matchesChord`).
 */
async function openAgents(page) {
  const entry = page.locator(css.overviewEntry)
  if ((await entry.count()) === 1 && (await entry.isVisible())) {
    await entry.click()
    return
  }
  await focusComposer(page)
  const mac = await page.evaluate(() => /Mac/.test(navigator.userAgent))
  await page.keyboard.press(mac ? keys.overview : "Control+Digit0")
}

function sentenceFailures(where, text) {
  const failures = []
  if (text !== unread)
    failures.push(`${where} says ${JSON.stringify(text)}, not ${JSON.stringify(unread)}`)
  if (text === unconfirmed) failures.push(`${where} says the unconfirmed-call sentence`)
  return failures
}

const widths = [
  { width: 1440, peek: "beside" },
  { width: 700, peek: "beneath" },
]

await main(
  {
    name: "conversation-unread",
    summary:
      "a conversation the window could not read says so in the transcript and the Agents peek",
    defaults: { engine: "chromium,webkit" },
    help: `
The gateway lists one running conversation and refuses conversation.read.
The transcript note and the Agents peek say what could not be read.

  beside    at 1440, the peek beside the list
  beneath   at 700, the peek opened under the Working row

A request row's unreadable is overview.test.tsx's: a first failed read never
lands in Needs you.`,
  },
  async ({ options, rep, url }) => {
    const origin = new URL(url).origin
    const chosen = options.quick ? widths.slice(0, 1) : widths
    await withEngines(options, rep, async (engine, browser) => {
      for (const { width, peek } of chosen)
        await attempt(rep, { name: peek, engine, width }, async () => {
          const seen = { reads: 0, unexpected: [] }
          let opened
          try {
            opened = await openPage(browser, {
              url: `${origin}/desktop.html`,
              width,
              height: 900,
              initScripts: [
                [gatewayHost, { endpoint: fakeGateway, credential: "fixture-only" }],
              ],
              readySelector: css.transcriptNote,
              beforeLoad: async (context) => {
                await context.routeWebSocket(`${fakeGateway}/**`, unreadGateway(seen))
              },
            })
            const { page } = opened
            const note = await noteOf(page)
            const failures = sentenceFailures("the transcript", note.text)
            if (!note.note || !note.chat)
              failures.push("no transcript note in the chat area")
            else if (!inside(note.note, note.chat))
              failures.push("the transcript note is outside the chat area")
            if (seen.reads < 1) failures.push("the conversation was never read")

            await openAgents(page)
            await need(page, css.overview, "the Agents overview")
            const failure = peek === "beside" ? css.overviewPeek : css.inlinePeek
            if (peek === "beneath") {
              await page.locator(css.overviewItem).first().click()
            }
            await need(
              page,
              `${failure} ${css.peekFailure}`,
              peek === "beside"
                ? "the peek beside the list"
                : "the peek beneath the Working row",
              10_000,
            )
            const said = await page.locator(`${failure} ${css.peekFailure}`).innerText()
            failures.push(
              ...sentenceFailures(
                peek === "beside"
                  ? "the peek beside the list"
                  : "the peek beneath the row",
                said,
              ),
            )
            if (seen.unexpected.length > 0)
              failures.push(`unexpected calls: ${seen.unexpected.join(", ")}`)
            return {
              failures,
              measured: { reads: seen.reads, note: note.text, peek: said },
            }
          } finally {
            await opened?.close().catch(() => {})
          }
        })
    })
  },
)
