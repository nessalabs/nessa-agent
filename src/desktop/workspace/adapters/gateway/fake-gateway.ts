/**
 * A gateway client for tests (`gateway-source.test.ts`): conversations held
 * in memory, every call recorded, and any call made to wait, fail, or answer
 * something else — the orderings the adapter's table on #248 names. Test
 * support only; nothing in the window imports it.
 */
import {
  NessaRpcError,
  type ConnectionState,
  type ConversationSummary,
  type ConversationView,
} from "@nessa/client"
import type { GatewayClient } from "./gateway-source"

type Method = "list" | "read" | "create" | "send" | "answer" | "archive"

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
    approvalModes: [],
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
          conversations: [...rows.values()],
          complete: fake.complete,
        })) as never,
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
