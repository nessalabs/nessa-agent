/**
 * A gateway client for tests (`gateway-source.test.ts`): conversations held
 * in memory, every call recorded, and any call made to wait, fail, or answer
 * something else — the orderings the adapter's table on #248 names. For
 * tests, and the browser fixture of an app's review
 * (`verification/desktop/fixtures/app-review/`).
 */
import {
  NessaRpcError,
  type ConnectionState,
  type ConversationSummary,
  type ConversationView,
} from "@nessa/client"
import type { GatewayClient } from "./gateway-source"

type Method = "list" | "observe" | "read" | "create" | "send" | "answer" | "archive"

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

/** A list row: what `conversation.list` says of one conversation. */
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

/** A conversation view: what `conversation.read` says, empty unless changed. */
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
   * When set, `list` returns only this many newest rows and is incomplete
   * once more are stored. Unset, `list` returns every row.
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
  /** The next call of `method` is answered by `behaviour` instead. */
  once(method: Method, behaviour: Behaviour): void
  /** Moves the connection to `state`, telling every observer. */
  setState(state: ConnectionState): void
  readonly closed: () => boolean
  readonly count: (method: Method) => number
}

export function fakeGateway(): FakeGateway {
  const calls: FakeGateway["calls"] = []
  const rows = new Map<string, ConversationSummary>()
  const views = new Map<string, ConversationView>()
  const overrides = new Map<Method, Behaviour[]>()
  const observers = new Set<(state: ConnectionState) => void>()
  let state: ConnectionState = { status: "connected" }
  let closed = false
  const fake = {
    complete: true,
    listLimit: null as number | null,
    observePageSize: null as number | null,
    observeStops: false,
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
  const client: GatewayClient = {
    conversation: {
      list: (...args) =>
        answer("list", args, () => ({
          conversations: listedRows(),
          complete:
            fake.listLimit !== null && rows.size > fake.listLimit ? false : fake.complete,
        })) as never,
      observe: (options) =>
        answer("observe", [options], () => observePage(options)) as never,
      read: (id) => answer("read", [id], () => views.get(id) ?? notFound()) as never,
      create: (options) =>
        answer("create", [options], () => ({
          conversationId: options?.conversationId ?? "",
        })) as never,
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
    get connectionState() {
      return state
    },
    onConnectionStateChange(handler) {
      observers.add(handler)
      return () => observers.delete(handler)
    },
    close() {
      closed = true
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
    setState(next: ConnectionState) {
      state = next
      for (const observer of [...observers]) observer(next)
    },
    closed: () => closed,
    count: (method: Method) => calls.filter((call) => call.method === method).length,
  })
}
