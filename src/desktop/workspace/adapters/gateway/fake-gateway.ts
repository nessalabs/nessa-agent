/**
 * A gateway client for tests (`gateway-source.test.ts`): conversations held
 * in memory, every call recorded, and any call made to wait, fail, or answer
 * something else — the orderings the adapter's tables on #248 and #702 name.
 * Its subscriptions behave as the gateway's (`docs/design/record-subscriptions.md`):
 * the reply, then a first frame; a frame whenever the test says the stored
 * conversation changed (`publish`, `publishList`); an end when the test says
 * (`end`); and every subscription ended `disconnected` when the connection is
 * lost, as `@nessa/client` does. For tests, and the browser fixtures of an
 * app's review and of message sync (`verification/desktop/fixtures/`).
 */
import {
  NessaRpcError,
  type ConnectionState,
  type ConversationListResult,
  type ConversationSummary,
  type ConversationView,
  type ConversationViewCursor,
  type ListSubscriptionHandlers,
  type SubscriptionEnd,
  type ViewSubscriptionHandlers,
} from "@nessa/client"
import type { GatewayClient } from "./gateway-source"

type Method =
  | "subscribe"
  | "subscribeList"
  | "unsubscribe"
  | "observe"
  | "create"
  | "send"
  | "answer"
  | "archive"

/** How one call is answered instead: given the normal answer, the promise to return. */
type Behaviour = (normal: () => unknown) => Promise<unknown>

export interface Deferred<T> {
  readonly promise: Promise<T>
  resolve(value: T): void
  reject(error: unknown): void
}

export function deferred<T = void>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => {
    resolve = ok
    reject = fail
  })
  return { promise, resolve, reject }
}

/** A list row: what a list frame says of one conversation. */
export function row(
  conversationId: string,
  change: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    conversationId,
    title: `Title of ${conversationId}`,
    preview: "Last said",
    createdAtMs: 1_000,
    updatedAtMs: 2_000,
    running: false,
    archived: false,
    ...change,
  }
}

/** A conversation view: what a view frame says, empty unless changed. */
export function view(
  conversationId: string,
  change: Partial<ConversationView> = {},
): ConversationView {
  return {
    conversationId,
    revision: "r1",
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        compactionReporting: "unsupported_not_implemented",
        modelSwitchReporting: "unsupported_not_implemented",
        permissionDeferral: "unsupported_not_implemented",
        elicitationForwarding: "unknown",
        preToolPolicy: "unknown",
        policyEndTurn: "unknown",
        policyCloseSession: "unknown",
        incomingElicitation: "unknown",
      },
    },
    lifecycle: { phase: "attached" },
    truncated: false,
    queueComplete: true,
    transcriptState: "complete",
    title: null,
    questions: [],
    approvalMode: "ask",
    approvalModes: [{ id: "ask", name: "Ask", description: "Ask before tools." }],
    ...change,
  }
}

export interface FakeGateway {
  readonly client: GatewayClient
  /** Every call made, in order. */
  readonly calls: { readonly method: Method; readonly args: readonly unknown[] }[]
  readonly rows: Map<string, ConversationSummary>
  readonly views: Map<string, ConversationView>
  /** Whether a list names every conversation. */
  complete: boolean
  /**
   * When set, a list frame carries only this many newest rows and is
   * incomplete once more are stored. Unset, it carries every row.
   */
  listLimit: number | null
  /**
   * When set, `observe` pages the rows in insertion order, this many at a
   * time, and the last page is complete. Unset, an incomplete list's observe
   * returns the current rows with `complete: false` and no cursor.
   */
  observePageSize: number | null
  /** An observe page answers `complete: false` and no cursor, so the pass stops. */
  observeStops: boolean
  /** The stored history's incarnation: every cursor names it. */
  incarnation: string
  /** The next call of `method` is answered by `behaviour` instead. */
  once(method: Method, behaviour: Behaviour): void
  /**
   * The conversation's stored history moved on: its position is the next,
   * and each live subscription to it is sent its view at that cursor.
   */
  publish(conversationId: string): void
  /** The list changed: each live list subscription is sent the list. */
  publishList(): void
  /** Ends every live subscription to `target` (a conversation id, or `"list"`) with `end`. */
  end(target: string, end: SubscriptionEnd): void
  /** The live subscriptions to `target` (a conversation id, or `"list"`). */
  live(target: string): number
  /** The cursor a frame of the conversation would carry now. */
  cursor(conversationId: string): ConversationViewCursor
  /** Sets the conversation's stored position, sending nothing: a history that went back. */
  rewind(conversationId: string, position: number): void
  /** Moves the connection to `state`, telling every observer; any but connected ends every subscription. */
  setState(state: ConnectionState): void
  readonly closed: () => boolean
  readonly count: (method: Method) => number
}

/** One live subscription: its target, and how its frames and end are told. */
type Live =
  | {
      readonly target: string
      readonly kind: "view"
      readonly handlers: ViewSubscriptionHandlers
    }
  | {
      readonly target: "list"
      readonly kind: "list"
      readonly handlers: ListSubscriptionHandlers
    }

export function fakeGateway(): FakeGateway {
  const calls: FakeGateway["calls"] = []
  const rows = new Map<string, ConversationSummary>()
  const views = new Map<string, ConversationView>()
  const overrides = new Map<Method, Behaviour[]>()
  const observers = new Set<(state: ConnectionState) => void>()
  const positions = new Map<string, number>()
  const subscriptions = new Map<string, Live>()
  let state: ConnectionState = { status: "connected" }
  let closed = false
  let ids = 0
  const fake = {
    complete: true,
    listLimit: null as number | null,
    observePageSize: null as number | null,
    observeStops: false,
    incarnation: "history-1",
  }
  const listedRows = () => {
    const stored = [...rows.values()]
    if (fake.listLimit === null) return stored
    return [...stored]
      .sort(
        (left, right) =>
          right.updatedAtMs - left.updatedAtMs ||
          left.conversationId.localeCompare(right.conversationId),
      )
      .slice(0, fake.listLimit)
  }
  const listed = (): ConversationListResult => ({
    conversations: listedRows(),
    complete:
      fake.listLimit !== null && rows.size > fake.listLimit ? false : fake.complete,
  })
  const observePage = (options: { cursor?: { id: string } } | undefined) => {
    if (fake.observePageSize === null || fake.observeStops) {
      const stored = [...rows.values()]
      const page =
        fake.observeStops && fake.observePageSize !== null
          ? stored.slice(0, fake.observePageSize)
          : stored
      return { conversations: page, complete: false }
    }
    const stored = [...rows.values()]
    const cursorId = options?.cursor?.id
    const found =
      cursorId === undefined
        ? -1
        : stored.findIndex((row) => row.conversationId === cursorId)
    const start = found < 0 && cursorId !== undefined ? stored.length : found + 1
    const page = stored.slice(start, start + fake.observePageSize)
    const last = page[page.length - 1]
    const lastIndex = last === undefined ? -1 : stored.indexOf(last)
    const more = start + page.length < stored.length
    return {
      conversations: page,
      complete: !more,
      ...(more && last
        ? {
            cursor: {
              incarnation: "catalogue",
              boundary: "1",
              creation: String(9 + lastIndex),
              id: last.conversationId,
            },
          }
        : {}),
    }
  }
  const answer = (method: Method, args: readonly unknown[], normal: () => unknown) => {
    calls.push({ method, args })
    const behaviour = overrides.get(method)?.shift()
    return behaviour ? behaviour(normal) : Promise.resolve().then(normal)
  }
  const notFound = () => {
    throw new NessaRpcError("conversation_not_found", "No such conversation")
  }
  const cursor = (conversationId: string): ConversationViewCursor => ({
    incarnation: fake.incarnation,
    position: String(positions.get(conversationId) ?? 1),
  })
  // A frame of a subscription still live, in a later task than the reply, as
  // a socket delivers it.
  const deliver = (id: string, send: (live: Live) => void) => {
    void Promise.resolve()
      .then(() => undefined)
      .then(() => {
        const live = subscriptions.get(id)
        if (live) send(live)
      })
  }
  const sendView = (id: string) =>
    deliver(id, (live) => {
      if (live.kind !== "view") return
      const held = views.get(live.target)
      if (held) live.handlers.view({ cursor: cursor(live.target), view: held })
    })
  const sendList = (id: string) =>
    deliver(id, (live) => {
      if (live.kind === "list") live.handlers.list(listed())
    })
  /** Takes `live` as a new subscription, and returns its handle. */
  const opened = (live: Live) => {
    const id = `sub-${++ids}`
    subscriptions.set(id, live)
    return {
      id,
      close: () => {
        if (subscriptions.get(id) !== live) return Promise.resolve()
        subscriptions.delete(id)
        return answer("unsubscribe", [id], () => undefined).then(() => undefined)
      },
    }
  }
  const endAll = (target: string | undefined, end: SubscriptionEnd) => {
    for (const [id, live] of [...subscriptions])
      if (target === undefined || live.target === target) {
        subscriptions.delete(id)
        live.handlers.ended(end)
      }
  }
  const client: GatewayClient = {
    conversation: {
      observe: (options) =>
        answer("observe", [options], () => observePage(options)) as never,
      create: (options) =>
        answer("create", [options], () => {
          const conversationId = options?.conversationId ?? ""
          if (!views.has(conversationId))
            views.set(conversationId, view(conversationId, { revision: "created" }))
          return { conversationId }
        }) as never,
      send: (...args) =>
        answer("send", args, () => ({
          executionId: args[4]?.executionId ?? "",
          disposition: "queued",
          requestId: args[4]?.requestId ?? "",
        })) as never,
      answer: (...args) =>
        answer("answer", args, () => ({ requestId: "answer", applied: true })) as never,
      archive: (...args) =>
        answer("archive", args, () => {
          rows.delete(args[0])
          return { requestId: "archive", applied: true }
        }) as never,
    },
    subscriptions: {
      view: (conversationId, handlers, options = {}) =>
        answer("subscribe", [conversationId, options], () => {
          if (!views.has(conversationId)) notFound()
          const after = options.after
          if (
            after &&
            after.incarnation === fake.incarnation &&
            BigInt(after.position) > BigInt(cursor(conversationId).position)
          )
            throw new NessaRpcError("cursor_ahead", "after names a later position")
          const handle = opened({ target: conversationId, kind: "view", handlers })
          sendView(handle.id)
          return handle
        }) as never,
      list: (handlers, options = {}) =>
        answer("subscribeList", [options], () => {
          const handle = opened({ target: "list", kind: "list", handlers })
          sendList(handle.id)
          return handle
        }) as never,
    },
    get connectionState() {
      return state
    },
    onConnectionStateChange(handler) {
      observers.add(handler)
      return () => observers.delete(handler)
    },
    close() {
      closed = true
      endAll(undefined, { reason: "disconnected" })
    },
  }
  return Object.assign(fake, {
    client,
    calls,
    rows,
    views,
    once(method: Method, behaviour: Behaviour) {
      overrides.set(method, [...(overrides.get(method) ?? []), behaviour])
    },
    publish(conversationId: string) {
      positions.set(conversationId, (positions.get(conversationId) ?? 1) + 1)
      for (const [id, live] of subscriptions)
        if (live.kind === "view" && live.target === conversationId) sendView(id)
    },
    publishList() {
      for (const [id, live] of subscriptions) if (live.kind === "list") sendList(id)
    },
    end(target: string, end: SubscriptionEnd) {
      endAll(target, end)
    },
    live: (target: string) =>
      [...subscriptions.values()].filter((live) => live.target === target).length,
    cursor,
    rewind(conversationId: string, position: number) {
      positions.set(conversationId, position)
    },
    setState(next: ConnectionState) {
      state = next
      // The client ends its subscriptions before anyone hears the state.
      if (next.status !== "connected") endAll(undefined, { reason: "disconnected" })
      for (const observer of [...observers]) observer(next)
    },
    closed: () => closed,
    count: (method: Method) => calls.filter((call) => call.method === method).length,
  })
}
