import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { agentOperationTimeoutMs, type NessaClient } from "@nessa/client"
// Derived, not restated: a literal here stops tracking the table the moment the
// table moves, and the whole point is that one table decides both sides.
import budgets from "../../../../protocol/defaults/agent-startup-budgets.json"
// The real transport and the real conversation API, not the package's public
// entry point: what is under test is the wiring between them, so neither can
// be a double.
import { WireSession } from "../../../../packages/nessa-client/src/transport/wire-session"
import { createConversationApi } from "../../../../packages/nessa-client/src/presentation/conversation-api"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { gatewayEffects } from "../gateway/effects"

/** No upload backoff belongs in a text-only send: waiting here is the failure. */
const unexpectedWait = () => Promise.reject(new Error("no wait expected"))
import { textContent } from "../../model"
import { conversationNotice } from "../../ui/notification"
import { sendDraft } from "./slice"

/**
 * The whole delivery path for a cold start, because each half of it was right
 * on its own: the gateway reports `agent_startup_deadline`, and the client
 * turns that into a notice — but the client used to abandon the request at 30 s
 * while the gateway was still spending 45 s on the handshake and up to 5 s more
 * tearing the failed process down. The typed answer then arrived for a request
 * that no longer existed and `dispatchFrame` dropped it, so the user was told
 * "Conversation command failed" for a failure the gateway had described
 * exactly.
 *
 * Nothing is mocked between the socket and the notice: a real `WireSession`, a
 * real conversation API, the real error wrapper, the real store, and the real
 * notice.
 */
const SERVER_WORST_CASE_MS =
  budgets.agent.startupMs + budgets.agent.shutdownGraceMs + budgets.agent.killTimeoutMs

function fakeSocket() {
  const sent: string[] = []
  const listeners = new Map<string, Set<(event: { data?: unknown }) => void>>()
  const socket = {
    readyState: 1,
    send(raw: string) {
      sent.push(raw)
    },
    addEventListener(type: string, handler: (event: { data?: unknown }) => void) {
      const set = listeners.get(type) ?? new Set()
      set.add(handler)
      listeners.set(type, set)
    },
    close() {},
  } as unknown as WebSocket
  const deliver = (raw: string) => {
    for (const handler of listeners.get("message") ?? []) handler({ data: raw })
  }
  return { socket, sent, deliver }
}

/** The request id the gateway would correlate its answer with. */
function requestId(raw: string): string {
  return (JSON.parse(raw) as { id: string }).id
}

beforeEach(() => {
  vi.useFakeTimers()
})
afterEach(() => {
  vi.useRealTimers()
})

it("tells the user the agent was still starting, after the gateway takes its full budget", async () => {
  const { socket, sent, deliver } = fakeSocket()
  const wire = new WireSession(socket)
  let next = 0
  const client = {
    conversation: createConversationApi(wire, () => `id-${next++}`),
  } as unknown as NessaClient
  const store = makeStore(
    createDependencies({ conversation: gatewayEffects(() => client, unexpectedWait) }),
  )

  const sending = store.dispatch(sendDraft({ content: textContent("first message") }))
  // Let `conversation.create` reach the socket before any time passes.
  await vi.advanceTimersByTimeAsync(0)
  expect(sent).toHaveLength(1)

  // Past the old 30 s default: the request must still be outstanding, or the
  // answer below has nothing to correlate with.
  await vi.advanceTimersByTimeAsync(SERVER_WORST_CASE_MS)
  deliver(
    JSON.stringify({
      type: "res",
      id: requestId(sent[0]!),
      ok: false,
      error: { code: "agent_startup_deadline", message: "agent_startup_deadline" },
    }),
  )
  await sending.catch(() => undefined)

  const tab = store.getState().conversation.conversations[0]!
  expect(conversationNotice(tab)).toMatchObject({
    title: "Agent was still starting",
  })
  // The gateway's `agent_startup_deadline`, as the one word the panel has for
  // it. No wire code reaches the tab.
  expect(tab.failure).toBe("agent-startup-deadline")
  expect(tab.turns[0]).toMatchObject({ receipt: "failed" })
})

it("outlasts the gateway's worst case rather than guessing a round number", () => {
  // The budget is derived from the same table the gateway reads, so this is
  // the relationship that has to hold, not the number it happens to produce.
  expect(agentOperationTimeoutMs).toBeGreaterThan(SERVER_WORST_CASE_MS)
})

/**
 * And the other direction: a gateway that really has stopped answering. The
 * client still gives up, and claims no more than it knows — the command was
 * admitted to the socket and never answered, so its delivery is unknown rather
 * than failed, and no typed rejection is invented for it.
 */
it("reports unknown delivery when the gateway never answers, inventing no reason", async () => {
  const { socket, sent, deliver } = fakeSocket()
  const wire = new WireSession(socket)
  let next = 0
  const client = {
    conversation: createConversationApi(wire, () => `id-${next++}`),
  } as unknown as NessaClient
  const store = makeStore(
    createDependencies({ conversation: gatewayEffects(() => client, unexpectedWait) }),
  )

  const sending = store.dispatch(sendDraft({ content: textContent("first message") }))
  await vi.advanceTimersByTimeAsync(0)
  // The conversation opens, so the message itself is the command left hanging.
  deliver(
    JSON.stringify({
      type: "res",
      id: requestId(sent[0]!),
      ok: true,
      payload: { conversationId: JSON.parse(sent[0]!).params.conversationId },
    }),
  )
  await vi.advanceTimersByTimeAsync(0)
  expect(sent).toHaveLength(2)

  await vi.advanceTimersByTimeAsync(agentOperationTimeoutMs + 1)
  await sending.catch(() => undefined)

  const tab = store.getState().conversation.conversations[0]!
  // No typed reason, because the gateway rejected nothing.
  expect(tab.failure).toBeUndefined()
  expect(tab.turns[0]).toMatchObject({ receipt: "unknown" })
  expect(conversationNotice(tab)).toMatchObject({
    title: "Delivery unknown",
    retry: { kind: "submission" },
  })
})
