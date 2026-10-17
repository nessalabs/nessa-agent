/**
 * A gateway client for tests (`gateway-source.test.ts`): conversations held
 * in memory, every call recorded, and any call made to wait, fail, or answer
 * something else — the orderings the adapter's tables on #248 and #702 name.
 * Its subscriptions behave as the gateway's (`docs/design/record-subscriptions.md`):
 * the reply, then a first frame; a frame whenever the test says the stored
 * conversation changed (`publish`, `publishList`); an end when the test says
 * (`end`); and every subscription ended `disconnected` when the connection is
 * lost, as `@nessa/client` does. An open given up on its way (its `signal`)
 * is closed once answered, and the next open of its target waits for that
 * close: the client's own gate (`createSubscriptionGate`), not a copy. For tests, and the browser fixtures of an
 * app's review and of message sync (`verification/desktop/fixtures/`).
 */
import {
  preparedSubmissionFields,
  NessaRpcError,
  createSubscriptionGate,
  subscriptionLimits,
  type ConnectionState,
  type ConversationApi,
  type ConversationListResult,
  type ConversationSummary,
  type ConversationView,
  type ConversationViewCursor,
  type ListSubscriptionHandlers,
  type Subscription,
  type SubscriptionEnd,
  type ViewSubscriptionHandlers,
} from "@nessa/client"
import type { GatewayClient } from "./gateway-source"
import type { SharingClient } from "./sharing-gateway"

/**
 * The fake's client: the source's, sharing's conversation methods (which
 * reject — no test of the source shares), and the permission-rule calls
 * Settings makes on the same conversation. The workspace adapter never calls
 * those. A window fake still has to answer, because the window's client
 * requires them.
 */
export type FakeGatewayClient = GatewayClient & {
  readonly conversation: GatewayClient["conversation"] &
    SharingClient["conversation"] &
    Pick<ConversationApi, "permissionRules" | "revokePermissionRule">
}

const notShared = () => Promise.reject(new Error("the fake gateway does not share"))

/** Canonical identities for named test conversations; display names stay separate. */
export const gatewayConversationIds = {
  a: "00000000-0000-4000-8000-000000000001",
  b: "00000000-0000-4000-8000-000000000002",
  s: "00000000-0000-4000-8000-000000000003",
  fast: "00000000-0000-4000-8000-000000000004",
  slow: "00000000-0000-4000-8000-000000000005",
} as const

type Method =
  | "subscribe"
  | "subscribeList"
  | "unsubscribe"
  | "observe"
  | "create"
  | "send"
  | "answer"
  | "answerQuestion"
  | "stop"
  | "withdraw"
  | "setApprovalMode"
  | "archive"
  | "permissionRules"
  | "revokePermissionRule"

type AnswerArgs = Parameters<ConversationApi["answer"]>
type AnswerResult = Awaited<ReturnType<ConversationApi["answer"]>>
type AnswerQuestionArgs = Parameters<ConversationApi["answerQuestion"]>
type AnswerQuestionResult = Awaited<ReturnType<ConversationApi["answerQuestion"]>>
type AnswerBehaviour = (
  normal: () => AnswerResult,
  args: AnswerArgs,
) => Promise<AnswerResult>
type Call =
  | { readonly method: "answer"; readonly args: AnswerArgs }
  | { readonly method: "answerQuestion"; readonly args: AnswerQuestionArgs }
  | {
      readonly method: Exclude<Method, "answer" | "answerQuestion">
      readonly args: readonly unknown[]
    }

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
    title: `Title of ${Object.entries(gatewayConversationIds).find(([, id]) => id === conversationId)?.[0] ?? conversationId}`,
    preview: "Last said",
    createdAtMs: 1_000,
    updatedAtMs: 2_000,
    running: false,
    archived: false,
    ...change,
  }
}

/**
 * A note somebody left in a conversation, as a view lists it: a note the
 * owner left from the desktop, only to be read, unless changed. A view
 * carries it in `notes` (`view(id, { notes: [note("n1")] })`).
 */
export function note(
  messageId: string,
  change: Partial<ConversationView["notes"][number]> = {},
): ConversationView["notes"][number] {
  return {
    messageId,
    text: `Note ${messageId}`,
    acceptedAtMs: 1_500,
    delivery: "record_only",
    deliveryState: "recorded",
    author: { kind: "owner", surface: "desktop" },
    ...change,
  }
}

/** The gateway's time each fake view is stamped with, unless changed. */
export const GATEWAY_NOW_MS = 1_700_000_000_000

/** A conversation view: what a view frame says, empty unless changed. */
export function view(
  conversationId: string,
  change: Partial<ConversationView> = {},
): ConversationView {
  return {
    conversationId,
    revision: "r1",
    serverNowMs: GATEWAY_NOW_MS,
    messages: [],
    pending: [],
    permissions: [],
    tools: [],
    artifacts: [],
    notes: [],
    unreadable: [],
    unreadableTruncated: false,
    capabilities: {
      queue: true,
      steer: true,
      resume: true,
      permissions: true,
      imageInput: false,
      agentFeatures: {
        permissionDenial: "supported_for_offered_permission_reviews",
        nativeHookSuppression: "unknown",
        nativeHooks: "off",
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
  readonly client: FakeGatewayClient
  /** Every call made, in order. */
  readonly calls: Call[]
  readonly rows: Map<string, ConversationSummary>
  readonly views: Map<string, ConversationView>
  /**
   * The execution each conversation's peek says is active, which a stop
   * names. Apart from the views on purpose: a view's message can still say
   * running after its turn stopped.
   */
  readonly active: Map<string, string>
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
  once(method: Exclude<Method, "answer">, behaviour: Behaviour): void
  /** Answer the next review with its captured SDK arguments, rather than a global last call. */
  onceAnswer(behaviour: AnswerBehaviour): void
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
  /** Delivers a server push to the handlers registered on the client. A push with no handler is dropped. */
  publishEvent(event: string, payload: unknown): void
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

export function fakeGateway(
  options: {
    /**
     * The gateway's own clock. When given, each view frame is stamped with it
     * as it is sent, as the gateway stamps each read; otherwise a view is sent
     * with the `serverNowMs` it was stored with.
     */
    readonly now?: () => number
  } = {},
): FakeGateway {
  const calls: FakeGateway["calls"] = []
  const rows = new Map<string, ConversationSummary>()
  const views = new Map<string, ConversationView>()
  const active = new Map<string, string>()
  const overrides = new Map<Exclude<Method, "answer">, Behaviour[]>()
  const answerOverrides: AnswerBehaviour[] = []
  const observers = new Set<(state: ConnectionState) => void>()
  const eventListeners = new Map<string, Set<(payload: unknown) => void>>()
  const positions = new Map<string, number>()
  const subscriptions = new Map<string, Live>()
  // View subscribes on their way, which the gateway counts against its limit.
  let openingViews = 0
  const gated = createSubscriptionGate()
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
  const answer = (
    method: Exclude<Method, "answer" | "answerQuestion">,
    args: readonly unknown[],
    normal: () => unknown,
  ) => {
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
      if (held)
        live.handlers.view({
          cursor: cursor(live.target),
          view: options.now ? { ...held, serverNowMs: options.now() } : held,
        })
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
  const client: FakeGatewayClient = {
    conversation: {
      share: notShared,
      unshare: notShared,
      shares: notShared,
      observe: (options) =>
        answer("observe", [options], () => observePage(options)) as never,
      create: (options) =>
        answer("create", [options], () => {
          const conversationId = options?.conversationId ?? ""
          if (!views.has(conversationId))
            views.set(
              conversationId,
              view(conversationId, {
                revision: "created",
                ...(options?.approvalMode ? { approvalMode: options.approvalMode } : {}),
              }),
            )
          return { conversationId }
        }) as never,
      submit: (prepared) => {
        preparedSubmissionFields(prepared)
        return answer(
          "send",
          [
            prepared.conversationId,
            prepared.text,
            prepared.attachments,
            prepared.files,
            { executionId: prepared.executionId, requestId: prepared.requestId },
          ],
          () => ({
            conversationId: prepared.conversationId,
            target: prepared.target,
            executionId: prepared.executionId,
            disposition: "queued",
            requestId: prepared.requestId,
          }),
        ) as never
      },
      answer: (...args) => {
        calls.push({ method: "answer", args })
        const normal = (): AnswerResult => ({ requestId: "answer", applied: true })
        const behaviour = answerOverrides.shift()
        return behaviour ? behaviour(normal, args) : Promise.resolve().then(normal)
      },
      answerQuestion: (...args) => {
        calls.push({ method: "answerQuestion", args })
        const normal = (): AnswerQuestionResult => {
          const [target] = args
          const current = views.get(target.conversationId)
          if (current)
            views.set(target.conversationId, {
              ...current,
              questions: current.questions.filter(
                (question) =>
                  question.executionId !== target.executionId ||
                  question.questionId !== target.questionId,
              ),
            })
          return { requestId: "answer-question", applied: true }
        }
        const behaviour = overrides.get("answerQuestion")?.shift()
        return (behaviour ? behaviour(normal) : Promise.resolve().then(normal)) as never
      },
      // A queued turn named is withdrawn: it ends cancelled and never runs.
      stop: (...args) =>
        answer("withdraw", args, () => {
          const [target] = args
          const current = views.get(target.conversationId)
          if (current)
            views.set(target.conversationId, {
              ...current,
              messages: current.messages.map((turn) =>
                turn.executionId === target.executionId
                  ? { ...turn, status: "cancelled" }
                  : turn,
              ),
              pending: current.pending.filter(
                (waiting) => waiting.executionId !== target.executionId,
              ),
            })
          return { requestId: "withdraw", stage: "accepted" }
        }) as never,
      // Committed as asked; the view says it from now on.
      setApprovalMode: (...args) =>
        answer("setApprovalMode", args, () => {
          const [conversationId, mode] = args
          const current = views.get(conversationId)
          if (current) views.set(conversationId, { ...current, approvalMode: mode })
          return { requestId: "set-approval-mode", mode }
        }) as never,
      archive: (...args) =>
        answer("archive", args, () => {
          rows.delete(args[0])
          return { requestId: "archive", applied: true }
        }) as never,
      permissionRules: () =>
        answer("permissionRules", [], () => ({ requestId: "rules", rules: [] })) as never,
      revokePermissionRule: (...args) =>
        answer("revokePermissionRule", args, () => ({
          requestId: "revoke",
          applied: false,
        })) as never,
    },
    // The peek names the active execution; it ends cancelled, with what it waited on.
    stopThread: (...args) =>
      answer("stop", args, () => {
        const [conversationId] = args
        const executionId = active.get(conversationId)
        if (executionId === undefined) return { outcome: "no_active_turn" as const }
        active.delete(conversationId)
        const current = views.get(conversationId)
        if (current)
          views.set(conversationId, {
            ...current,
            messages: current.messages.map((turn) =>
              turn.executionId === executionId ? { ...turn, status: "cancelled" } : turn,
            ),
            permissions: current.permissions.filter(
              (asked) => asked.executionId !== executionId,
            ),
            questions: current.questions.filter(
              (asked) => asked.executionId !== executionId,
            ),
          })
        return { requestId: "stop", stage: "accepted" }
      }) as never,
    subscriptions: {
      view: (conversationId, handlers, { signal, ...options } = {}) =>
        gated(`view:${conversationId}`, signal, () => {
          // As the gateway counts: a subscribe on its way holds its place
          // until it is answered, and one past the limit is refused.
          const inUse =
            openingViews +
            [...subscriptions.values()].filter((live) => live.kind === "view").length
          if (inUse >= subscriptionLimits.conversationTargets) {
            calls.push({ method: "subscribe", args: [conversationId, options] })
            return Promise.reject(
              new NessaRpcError("subscription_capacity", "Past the subscription limit"),
            )
          }
          openingViews += 1
          return (
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
            }) as Promise<Subscription>
          ).finally(() => {
            openingViews -= 1
          })
        }) as never,
      list: (handlers, { signal, ...options } = {}) =>
        gated(
          `list:${options.archived ?? false}`,
          signal,
          () =>
            answer("subscribeList", [options], () => {
              const handle = opened({ target: "list", kind: "list", handlers })
              sendList(handle.id)
              return handle
            }) as Promise<Subscription>,
        ) as never,
    },
    get connectionState() {
      return state
    },
    onConnectionStateChange(handler) {
      observers.add(handler)
      return () => observers.delete(handler)
    },
    onEvent(event: string, handler: (payload: unknown) => void) {
      const handlers = eventListeners.get(event) ?? new Set()
      handlers.add(handler)
      eventListeners.set(event, handlers)
      return () => {
        handlers.delete(handler)
      }
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
    active,
    once(method: Exclude<Method, "answer">, behaviour: Behaviour) {
      overrides.set(method, [...(overrides.get(method) ?? []), behaviour])
    },
    onceAnswer(behaviour: AnswerBehaviour) {
      answerOverrides.push(behaviour)
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
    publishEvent(event: string, payload: unknown) {
      const handlers = eventListeners.get(event)
      if (!handlers) return
      for (const handler of [...handlers]) handler(payload)
    },
    closed: () => closed,
    count: (method: Method) => calls.filter((call) => call.method === method).length,
  })
}
