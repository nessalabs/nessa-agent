import { describe, expect, it, vi } from "vitest"
import { createSubscriptionApi, type SubscriptionPort } from "./subscription-api.js"
import type { ConnectionState } from "../application/managed-session.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"
import {
  passiveReadTiming,
  ProductEvent,
  ProductMethod,
  type ConversationView,
} from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const cursor = { incarnation: "store", position: "7" }

const view: ConversationView = {
  conversationId,
  approvalMode: "ask",
  approvalModes: [{ id: "ask", name: "Provider asks", description: "Asks." }],
  title: null,
  revision: "opaque-1",
  messages: [],
  pending: [],
  permissions: [],
  questions: [],
  tools: [],
  capabilities: {
    queue: true,
    steer: true,
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
}

/** A managed session the test drives: what was asked, and events and states pushed in. */
function port(
  reply: (method: string, params: unknown) => unknown = () => ({ subscriptionId: "1" }),
) {
  const handlers = new Map<string, Set<(payload: unknown) => void>>()
  const states = new Set<(state: ConnectionState) => void>()
  const request = vi.fn(async (method: string, params: unknown) => reply(method, params))
  const session: SubscriptionPort = {
    request,
    onEvent: (event, handler) => {
      const set = handlers.get(event) ?? new Set()
      set.add(handler)
      handlers.set(event, set)
      return () => set.delete(handler)
    },
    onState: (handler) => {
      states.add(handler)
      return () => states.delete(handler)
    },
  }
  return {
    session,
    request,
    emit: (event: string, payload: unknown) => {
      for (const handler of handlers.get(event) ?? []) handler(payload)
    },
    state: (state: ConnectionState) => {
      for (const handler of states) handler(state)
    },
  }
}

function viewHandlers() {
  return { view: vi.fn(), ended: vi.fn() }
}

describe("subscription API", () => {
  it("subscribes with the passive deadline and hands each checked frame over", async () => {
    const wire = port()
    const api = createSubscriptionApi(wire.session)
    const handlers = viewHandlers()
    const subscription = await api.view(conversationId, handlers, { after: cursor })
    expect(subscription.id).toBe("1")
    expect(wire.request).toHaveBeenCalledExactlyOnceWith(
      ProductMethod.ConversationSubscribe,
      { conversationId, after: cursor },
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    expect(handlers.view).toHaveBeenCalledExactlyOnceWith({ cursor, view })
    // Another subscription's frame, or the wrong event for this one, is not its.
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "2", cursor, view })
    wire.emit(ProductEvent.ConversationListed, {
      subscriptionId: "1",
      list: { conversations: [], complete: true },
    })
    expect(handlers.view).toHaveBeenCalledOnce()
  })

  it("ends once on the gateway's end frame and takes nothing after it", async () => {
    const wire = port()
    const handlers = viewHandlers()
    await createSubscriptionApi(wire.session).view(conversationId, handlers)
    wire.emit(ProductEvent.ConversationSubscriptionEnded, {
      subscriptionId: "1",
      reason: "lagging",
      lastDelivered: cursor,
    })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    wire.emit(ProductEvent.ConversationSubscriptionEnded, {
      subscriptionId: "1",
      reason: "refused",
      code: "forbidden",
    })
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({
      reason: "lagging",
      lastDelivered: cursor,
    })
    expect(handlers.view).not.toHaveBeenCalled()
  })

  it("ends a subscription whose frame fails the read's checks, and lets it go", async () => {
    const wire = port()
    const handlers = viewHandlers()
    await createSubscriptionApi(wire.session).view(conversationId, handlers)
    wire.emit(ProductEvent.ConversationView, {
      subscriptionId: "1",
      cursor,
      view: { ...view, conversationId: "00000000-0000-4000-8000-000000000002" },
    })
    expect(handlers.view).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("delivers nothing once closed", async () => {
    const wire = port()
    const handlers = viewHandlers()
    const subscription = await createSubscriptionApi(wire.session).view(
      conversationId,
      handlers,
    )
    const closing = subscription.close()
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    await closing
    expect(handlers.view).not.toHaveBeenCalled()
    expect(handlers.ended).not.toHaveBeenCalled()
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("ends every subscription when its connection is lost, and a new connection's identity is not its", async () => {
    const wire = port()
    const api = createSubscriptionApi(wire.session)
    const handlers = viewHandlers()
    await api.view(conversationId, handlers)
    wire.state({
      status: "reconnecting",
      attempt: 1,
      error: new NessaConnectionClosedError(1006, "lost"),
    })
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "disconnected" })
    wire.state({ status: "connected" })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    expect(handlers.view).not.toHaveBeenCalled()
  })

  it("refuses a bad conversation or cursor before sending", async () => {
    const wire = port()
    const api = createSubscriptionApi(wire.session)
    await expect(api.view("bad", viewHandlers())).rejects.toThrow(
      "Invalid conversation ID",
    )
    await expect(
      api.view(conversationId, viewHandlers(), {
        after: { incarnation: "store", position: "-1" },
      }),
    ).rejects.toThrow("Invalid view cursor")
    expect(wire.request).not.toHaveBeenCalled()
  })

  it("follows the list with its archived filter checked on every frame", async () => {
    const wire = port()
    const handlers = { list: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).list(handlers, { archived: true })
    expect(wire.request).toHaveBeenCalledWith(
      ProductMethod.ConversationSubscribeList,
      { archived: true },
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
    const row = {
      conversationId,
      title: "t",
      preview: "p",
      createdAtMs: 1,
      updatedAtMs: 2,
      running: false,
      archived: true,
    }
    wire.emit(ProductEvent.ConversationListed, {
      subscriptionId: "1",
      list: { conversations: [row], complete: false },
    })
    expect(handlers.list).toHaveBeenCalledExactlyOnceWith({
      conversations: [row],
      complete: false,
    })
    wire.emit(ProductEvent.ConversationListed, {
      subscriptionId: "1",
      list: { conversations: [{ ...row, archived: false }], complete: true },
    })
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
  })
})
