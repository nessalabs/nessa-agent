import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { NessaClient } from "@nessa/client"
import { WireSession } from "../../../../packages/nessa-client/src/transport/wire-session"
import { createConversationApi } from "../../../../packages/nessa-client/src/presentation/conversation-api"
import { makeStore } from "../../../store"
import { createDependencies } from "../../../composition/dependencies"
import { textContent } from "../../model"
import { conversationNotice } from "../../ui/notification"
import { gatewayEffects } from "../gateway/effects"
import { sendDraft } from "./slice"

const unexpectedWait = () => Promise.reject(new Error("no wait expected"))

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

function requestId(raw: string): string {
  return (JSON.parse(raw) as { id: string }).id
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

it("uses the configured ordinary deadline when an admitted command never answers", async () => {
  const { socket, sent, deliver } = fakeSocket()
  const wire = new WireSession(socket, { requestTimeoutMs: 25 })
  let next = 0
  const client = {
    conversation: createConversationApi(wire, () => `id-${next++}`),
  } as unknown as NessaClient
  const store = makeStore(
    createDependencies({ conversation: gatewayEffects(() => client, unexpectedWait) }),
  )

  const sending = store.dispatch(sendDraft({ content: textContent("first message") }))
  await vi.advanceTimersByTimeAsync(0)
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

  await vi.advanceTimersByTimeAsync(26)
  await sending.catch(() => undefined)

  const tab = store.getState().conversation.conversations[0]!
  expect(tab.failure).toBeUndefined()
  expect(tab.turns[0]).toMatchObject({ receipt: "unknown" })
  expect(conversationNotice(tab)).toMatchObject({
    title: "Delivery unknown",
    retry: { kind: "submission" },
  })
})
