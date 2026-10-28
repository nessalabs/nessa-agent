import { describe, expect, it, vi } from "vitest"
import { createSubscriptionApi, type SubscriptionPort } from "./subscription-api.js"
import type { ConnectionState } from "../application/managed-session.js"
import { endedFrame } from "../protocol/subscription-validate.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"
import {
  passiveReadTiming,
  ProductEvent,
  ProductMethod,
  subscriptionLimits,
  eventSubscriptionBounds,
  type ConversationView,
} from "../generated/product.js"

const conversationId = "00000000-0000-4000-8000-000000000001"
const cursor = { incarnation: "store", position: "7" }

const view: ConversationView = {
  conversationId,
  artifacts: [],
  notes: [],
  unreadable: [],
  unreadableTruncated: false,
  handoffs: [],
  approvalMode: "ask",
  model: "claude-sonnet-5",
  approvalModes: [{ id: "ask", name: "Provider asks", description: "Asks." }],
  title: null,
  revision: "opaque-1",
  serverNowMs: 1_700_000_000_000,
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
      nativeHooks: "off",
      compactionReporting: "unsupported_not_implemented",
      modelSwitchReporting: "unsupported_not_implemented",
      permissionDeferral: "unsupported_not_implemented",
      elicitationForwarding: "unknown",
      preToolPolicy: "unsupported_not_implemented",
      policyEndTurn: "unsupported",
      policyCloseSession: "unsupported",
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
  for (const early of [false, true]) {
    for (const kind of ["turn.ended", "tool.updated"]) {
      it(`E24 rejects incomplete ${kind} metadata ${early ? "before" : "after"} open reply without publishing a checkpoint`, async () => {
        const malformed = {
          subscriptionId: "1",
          cursor: "checkpoint",
          events: [{ id: "one", cursor: "one", kind }],
        }
        const wire = port((method) => {
          if (early && method === ProductMethod.ConversationSubscribeEvents)
            wire.emit(ProductEvent.ConversationEvents, malformed)
          return { subscriptionId: "1" }
        })
        const handlers = { batch: vi.fn(), ended: vi.fn() }
        const handle = await createSubscriptionApi(wire.session).events(
          conversationId,
          handlers,
        )
        if (!early) wire.emit(ProductEvent.ConversationEvents, malformed)
        expect(handlers.batch).not.toHaveBeenCalled()
        expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({
          reason: "invalid_frame",
        })
        await handle.close()
        expect(
          wire.request.mock.calls.filter(
            ([method]) => method === ProductMethod.ConversationUnsubscribe,
          ),
        ).toHaveLength(1)
      })
    }
  }

  it("rejects terminal frames combining view and event cursors", () => {
    expect(() =>
      endedFrame({
        subscriptionId: "1",
        reason: "lagging",
        lastDelivered: cursor,
        lastEventCursor: "event-cursor",
      }),
    ).toThrow(TypeError)
  })

  for (const kind of ["events", "view", "list", "agents"] as const) {
    for (const early of [false, true]) {
      for (const field of ["lastDelivered", "lastEventCursor", "both"] as const) {
        const valid =
          (kind === "events" && field === "lastEventCursor") ||
          (kind === "view" && field === "lastDelivered")
        it(`${valid ? "accepts" : "rejects"} terminal cursor evidence for ${kind}, ${field}, ${early ? "before" : "after"} open reply`, async () => {
          const terminal = {
            subscriptionId: "1",
            reason: "lagging",
            ...(field === "lastDelivered" || field === "both"
              ? { lastDelivered: cursor }
              : {}),
            ...(field === "lastEventCursor" || field === "both"
              ? { lastEventCursor: "event-cursor" }
              : {}),
          }
          const wire = port((method) => {
            if (early && method !== ProductMethod.ConversationUnsubscribe)
              wire.emit(ProductEvent.ConversationSubscriptionEnded, terminal)
            return { subscriptionId: "1" }
          })
          const api = createSubscriptionApi(wire.session)
          const handlers = {
            batch: vi.fn(),
            view: vi.fn(),
            list: vi.fn(),
            catalog: vi.fn(),
            ended: vi.fn(),
          }
          const opened = await (kind === "events"
            ? api.events(conversationId, handlers)
            : kind === "view"
              ? api.view(conversationId, handlers)
              : kind === "list"
                ? api.list(handlers)
                : api.agents(handlers))
          if (!early) wire.emit(ProductEvent.ConversationSubscriptionEnded, terminal)
          const { subscriptionId: _, ...end } = terminal
          expect(handlers.ended).toHaveBeenCalledExactlyOnceWith(
            valid ? end : { reason: "invalid_frame" },
          )
          wire.emit(ProductEvent.ConversationSubscriptionEnded, terminal)
          expect(handlers.ended).toHaveBeenCalledOnce()
          await opened.close()
          const stops = wire.request.mock.calls.filter(
            ([method]) =>
              method === ProductMethod.ConversationUnsubscribe ||
              method === ProductMethod.AgentsUnsubscribe,
          )
          expect(stops).toHaveLength(valid ? 0 : 1)
        })
      }
    }
  }

  it("bounds unknown subscription identities and ends every affected open before releasing its actual handle", async () => {
    const replies: ((reply: { subscriptionId: string }) => void)[] = []
    let admitted!: () => void
    const bothAdmitted = new Promise<void>((resolve) => {
      admitted = resolve
    })
    const wire = port((method) =>
      method !== ProductMethod.ConversationSubscribeEvents
        ? {}
        : new Promise((resolve) => {
            replies.push(resolve)
            if (replies.length === 2) admitted()
          }),
    )
    const api = createSubscriptionApi(wire.session)
    const first = { batch: vi.fn(), ended: vi.fn() }
    const second = { batch: vi.fn(), ended: vi.fn() }
    const opens = [
      api.events(conversationId, first),
      api.events("00000000-0000-4000-8000-000000000002", second),
    ]
    await bothAdmitted
    for (
      let count = 0;
      count < subscriptionLimits.conversationTargets + subscriptionLimits.listTargets + 2;
      count++
    ) {
      // Empty filtered batches still introduce identities and must be bounded.
      wire.emit(ProductEvent.ConversationEvents, {
        subscriptionId: String(count + 100),
        cursor: "one",
        events: [],
      })
    }
    replies[0]({ subscriptionId: "1" })
    replies[1]({ subscriptionId: "2" })
    const handles = await Promise.all(opens)
    expect(first.ended).toHaveBeenCalledExactlyOnceWith({ reason: "lagging" })
    expect(second.ended).toHaveBeenCalledExactlyOnceWith({ reason: "lagging" })
    expect(first.batch).not.toHaveBeenCalled()
    expect(second.batch).not.toHaveBeenCalled()
    for (const subscriptionId of ["1", "2"])
      expect(wire.request).toHaveBeenCalledWith(ProductMethod.ConversationUnsubscribe, {
        subscriptionId,
      })
    await Promise.all(handles.map((handle) => handle.close()))
    expect(
      wire.request.mock.calls.filter(
        ([method]) => method === ProductMethod.ConversationUnsubscribe,
      ),
    ).toHaveLength(2)
  })

  it("validates unknown early event payloads before retaining them and closes the real reply identity", async () => {
    const wire = port(() => {
      wire.emit(ProductEvent.ConversationEvents, {
        subscriptionId: "999",
        cursor: "one",
        events: [
          {
            id: "one",
            cursor: "one",
            kind: "state.changed",
            status: "opened",
            name: "x".repeat(eventSubscriptionBounds.maxNameCharacters + 1),
          },
        ],
      })
      return { subscriptionId: "1" }
    })
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).events(conversationId, handlers)
    expect(handlers.batch).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("bounds total early batches across different unknown identities", async () => {
    const wire = port((method) => {
      if (method === ProductMethod.ConversationSubscribeEvents) {
        for (let count = 0; count < 18; count++)
          wire.emit(ProductEvent.ConversationEvents, {
            subscriptionId: count % 2 === 0 ? "998" : "999",
            cursor: String(count),
            events: [],
          })
      }
      return { subscriptionId: "1" }
    })
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).events(conversationId, handlers)
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "lagging" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("follows semantic batches in order without coalescing early frames", async () => {
    const wire = port(() => {
      for (const cursor of ["one", "two"])
        wire.emit(ProductEvent.ConversationEvents, {
          subscriptionId: "1",
          cursor,
          events: [{ id: cursor, cursor, kind: "state.changed", status: "opened" }],
        })
      return { subscriptionId: "1" }
    })
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    const subscription = await createSubscriptionApi(wire.session).events(
      conversationId,
      handlers,
      { replay: true },
    )
    expect(handlers.batch.mock.calls.map(([batch]) => batch.cursor)).toEqual([
      "one",
      "two",
    ])
    expect(wire.request).toHaveBeenCalledWith(
      ProductMethod.ConversationSubscribeEvents,
      { conversationId, replay: true },
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
    await subscription.close()
  })

  it("terminates and releases an event subscription with malformed metadata", async () => {
    const wire = port()
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).events(conversationId, handlers)
    wire.emit(ProductEvent.ConversationEvents, {
      subscriptionId: "1",
      cursor: "one",
      events: [
        {
          id: "one",
          cursor: "one",
          kind: "tool.updated",
          executionId: "turn",
          toolId: "tool",
          name: "x".repeat(1025),
        },
      ],
    })
    expect(handlers.batch).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("rejects contradictory event identity before publishing a batch", async () => {
    const wire = port()
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).events(conversationId, handlers)
    wire.emit(ProductEvent.ConversationEvents, {
      subscriptionId: "1",
      cursor: "checkpoint",
      events: [
        { id: "identity", cursor: "different", kind: "state.changed", status: "opened" },
      ],
    })
    expect(handlers.batch).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("fails explicitly when early event batches exceed the bounded opening buffer", async () => {
    const wire = port(() => {
      for (let count = 0; count < 100; count++)
        wire.emit(ProductEvent.ConversationEvents, {
          subscriptionId: "1",
          cursor: String(count),
          events: [],
        })
      return { subscriptionId: "1" }
    })
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).events(conversationId, handlers)
    expect(handlers.batch).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "lagging" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("rejects malformed replay options before sending", async () => {
    const wire = port()
    const handlers = { batch: vi.fn(), ended: vi.fn() }
    const api = createSubscriptionApi(wire.session)
    await expect(
      api.events(conversationId, handlers, { after: "one", replay: true }),
    ).rejects.toBeInstanceOf(TypeError)
    await expect(
      api.events(conversationId, handlers, { kinds: ["state.changed", "state.changed"] }),
    ).rejects.toBeInstanceOf(TypeError)
    expect(wire.request).not.toHaveBeenCalled()
  })

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

  it("follows the agent catalog, checked as agents.list is, and stops it with agents.unsubscribe", async () => {
    const wire = port()
    const handlers = { catalog: vi.fn(), ended: vi.fn() }
    const subscription = await createSubscriptionApi(wire.session).agents(handlers)
    expect(wire.request).toHaveBeenCalledWith(
      ProductMethod.AgentsSubscribe,
      {},
      { atLeastMs: passiveReadTiming.minRequestTimeoutMs },
    )
    const catalog = {
      agents: [],
      environments: ["build.example", `peer:${"ab".repeat(32)}`],
      commandHosts: ["build.example"],
      nativeHooks: { claude: "off", codex: "on" },
    }
    wire.emit(ProductEvent.AgentsListed, { subscriptionId: "1", catalog })
    expect(handlers.catalog).toHaveBeenCalledExactlyOnceWith(catalog)
    await subscription.close()
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.AgentsUnsubscribe, {
      subscriptionId: "1",
    })
    wire.emit(ProductEvent.AgentsListed, { subscriptionId: "1", catalog })
    expect(handlers.catalog).toHaveBeenCalledOnce()
  })

  it("ends an agent catalog whose frame fails the catalog's checks, stopping it with its own unsubscribe", async () => {
    const wire = port()
    const handlers = { catalog: vi.fn(), ended: vi.fn() }
    await createSubscriptionApi(wire.session).agents(handlers)
    wire.emit(ProductEvent.AgentsListed, {
      subscriptionId: "1",
      catalog: {
        agents: [],
        environments: ["same", "same"],
        commandHosts: [],
        nativeHooks: { claude: "off", codex: "off" },
      },
    })
    expect(handlers.catalog).not.toHaveBeenCalled()
    expect(handlers.ended).toHaveBeenCalledExactlyOnceWith({ reason: "invalid_frame" })
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.AgentsUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("closes an open given up on its way once answered, and sends the next open of that conversation only after", async () => {
    // The gateway refuses a second live subscription to one conversation, so a
    // follower that stops and follows again must not race its own first open.
    const replies: Array<{ method: string; done: (value: unknown) => void }> = []
    const wire = port(
      (method) =>
        new Promise((done) => {
          replies.push({ method, done })
        }),
    )
    const api = createSubscriptionApi(wire.session)
    const stopped = new AbortController()
    const first = api
      .view(conversationId, viewHandlers(), { signal: stopped.signal })
      .catch((error: unknown) => error)
    // Sent, and given up on before the gateway answers.
    await vi.waitFor(() => expect(replies).toHaveLength(1))
    stopped.abort()
    const second = api.view(conversationId, viewHandlers())
    // Another conversation does not wait on this one.
    const other = api.view("00000000-0000-4000-8000-000000000002", viewHandlers())
    await Promise.resolve()
    await Promise.resolve()
    const methods = () => replies.map((reply) => reply.method)
    expect(methods()).toEqual([
      ProductMethod.ConversationSubscribe,
      ProductMethod.ConversationSubscribe,
    ])
    replies[1]!.done({ subscriptionId: "3" })
    expect(await other).toMatchObject({ id: "3" })
    replies[0]!.done({ subscriptionId: "1" })
    await vi.waitFor(() => expect(replies).toHaveLength(3))
    expect(methods()[2]).toBe(ProductMethod.ConversationUnsubscribe)
    expect(wire.request.mock.calls[2]![1]).toEqual({ subscriptionId: "1" })
    // Still closing: the second open waits.
    await Promise.resolve()
    expect(replies).toHaveLength(3)
    replies[2]!.done({})
    await vi.waitFor(() => expect(replies).toHaveLength(4))
    expect(methods()[3]).toBe(ProductMethod.ConversationSubscribe)
    replies[3]!.done({ subscriptionId: "2" })
    expect(await second).toMatchObject({ id: "2" })
    expect(await first).toMatchObject({ name: "AbortError" })
  })

  it("closes a subscription when its signal is aborted after the answer", async () => {
    const wire = port()
    const stopped = new AbortController()
    const handlers = viewHandlers()
    await createSubscriptionApi(wire.session).view(conversationId, handlers, {
      signal: stopped.signal,
    })
    stopped.abort()
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    expect(handlers.view).not.toHaveBeenCalled()
  })

  it("A1: hands over the frames that come in the same turn as the subscribe's answer", async () => {
    // The transport resolves the answer and dispatches the next frames in one
    // turn, as a socket that reads several messages at once does: the open
    // has not registered the identity yet when they arrive.
    const answers: Array<(value: unknown) => void> = []
    const wire = port(
      () =>
        new Promise((done) => {
          answers.push(done)
        }),
    )
    const api = createSubscriptionApi(wire.session)
    const handlers = viewHandlers()
    const opened = api.view(conversationId, handlers)
    const ended = viewHandlers()
    const endedOpen = api.view("00000000-0000-4000-8000-000000000002", ended)
    await vi.waitFor(() => expect(answers).toHaveLength(2))
    const later = { incarnation: "store", position: "8" }
    answers[0]!({ subscriptionId: "1" })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor: later, view })
    // Another identity's frame is claimed by nobody.
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "9", cursor, view })
    answers[1]!({ subscriptionId: "2" })
    wire.emit(ProductEvent.ConversationSubscriptionEnded, {
      subscriptionId: "2",
      reason: "refused",
      code: "forbidden",
    })
    expect(await opened).toMatchObject({ id: "1" })
    await endedOpen
    // A frame is a replacement: the latest is what the caller is handed.
    expect(handlers.view).toHaveBeenCalledExactlyOnceWith({ cursor: later, view })
    expect(ended.ended).toHaveBeenCalledExactlyOnceWith({
      reason: "refused",
      code: "forbidden",
    })
    // Nothing is held once no open is on its way.
    answers.length = 0
    const third = viewHandlers()
    const next = api.view("00000000-0000-4000-8000-000000000003", third)
    await vi.waitFor(() => expect(answers).toHaveLength(1))
    answers[0]!({ subscriptionId: "9" })
    await next
    expect(third.view).not.toHaveBeenCalled()
  })

  it("A1: hands nothing of an open given up before its answer, though its frame came with it", async () => {
    const answers: Array<(value: unknown) => void> = []
    const wire = port((method) =>
      method === ProductMethod.ConversationUnsubscribe
        ? {}
        : new Promise((done) => {
            answers.push(done)
          }),
    )
    const stopped = new AbortController()
    const handlers = viewHandlers()
    const opened = createSubscriptionApi(wire.session)
      .view(conversationId, handlers, { signal: stopped.signal })
      .catch((error: unknown) => error)
    await vi.waitFor(() => expect(answers).toHaveLength(1))
    stopped.abort()
    answers[0]!({ subscriptionId: "1" })
    wire.emit(ProductEvent.ConversationView, { subscriptionId: "1", cursor, view })
    expect(await opened).toMatchObject({ name: "AbortError" })
    expect(handlers.view).not.toHaveBeenCalled()
    expect(wire.request).toHaveBeenLastCalledWith(ProductMethod.ConversationUnsubscribe, {
      subscriptionId: "1",
    })
  })

  it("leaves no listener on its signal once closed", async () => {
    // A long-lived signal across many opens would otherwise collect one
    // listener for each.
    const wire = port()
    const stopped = new AbortController()
    const added = vi.spyOn(stopped.signal, "addEventListener")
    const removed = vi.spyOn(stopped.signal, "removeEventListener")
    const subscription = await createSubscriptionApi(wire.session).view(
      conversationId,
      viewHandlers(),
      { signal: stopped.signal },
    )
    await subscription.close()
    const listener = added.mock.calls.find(([type]) => type === "abort")?.[1]
    expect(listener).toBeDefined()
    expect(removed).toHaveBeenCalledWith("abort", listener)
    stopped.abort()
    const unsubscribes = wire.request.mock.calls.filter(
      ([method]) => method === ProductMethod.ConversationUnsubscribe,
    )
    expect(unsubscribes).toHaveLength(1)
  })
})
