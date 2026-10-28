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
  viewEntryBounds,
  type BoatVmLifetime,
  type BoatRuntimeSettings,
  type Channel as WireChannel,
  type ChannelCreateParams,
  type ConnectionState,
  type ConversationApi,
  type ConversationListResult,
  type ConversationPermission,
  type ConversationQuestion,
  type ConversationQuestionEntry,
  type ConversationReviewEntry,
  type ConversationSummary,
  type ConversationView,
  type ConversationViewCursor,
  type ListSubscriptionHandlers,
  type Subscription,
  type SubscriptionEnd,
  type ViewSubscriptionHandlers,
  type WorkspaceWindow,
} from "@nessa/client"
import type { GatewayClient } from "./gateway-source"
import type { SharingClient } from "./sharing-gateway"
import type { WindowPublishClient } from "./window-publisher"

/**
 * The fake's client: the source's, sharing's conversation methods (which
 * reject — no test of the source shares), and the permission-rule calls
 * Settings makes on the same conversation. The workspace adapter never calls
 * those. A window fake still has to answer, because the window's client
 * requires them.
 */
export type FakeGatewayClient = {
  readonly boat: import("@nessa/client").BoatApi
} & GatewayClient &
  WindowPublishClient & {
    readonly conversation: GatewayClient["conversation"] &
      SharingClient["conversation"] &
      Pick<ConversationApi, "permissionRules" | "revokePermissionRule">
  }

/** The default channel, as the gateway lists it first. */
const defaultChannel: WireChannel = {
  channelId: "default",
  name: "Conversations",
  folder: "/workspace",
  folderMade: false,
  agent: null,
  model: null,
  approvalMode: "ask",
  coordinatorConversationId: null,
  coordinatorPlacement: "local",
  threadPlacement: "local",
  createdAtMs: 0,
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
  | "permissionReview"
  | "questionReview"
  | "stop"
  | "withdraw"
  | "setApprovalMode"
  | "setUsageLimitAutoContinue"
  | "setModel"
  | "switchAgent"
  | "fork"
  | "read"
  | "archive"
  | "rename"
  | "delete"
  | "permissionRules"
  | "revokePermissionRule"
  | "listChannels"
  | "createChannel"

/** A turn's reply as a fork copies it: its text, joined. */
function turnReply(turn: ConversationView["messages"][number]): string | undefined {
  const text = turn.parts
    .flatMap((part) => (part.kind === "text" ? [part.text] : []))
    .join("")
  return text || undefined
}

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

/**
 * What a review or question shows, its key optional: a fake view lists it
 * under the key it makes for it (`reviewKey`, `questionKey`).
 */
export type Unkeyed<T extends { review: string } | { question: string }> = Omit<
  T,
  "review" | "question"
> &
  Partial<Pick<T, keyof T & ("review" | "question")>>

/** The gateway's time each fake view is stamped with, unless changed. */
export const GATEWAY_NOW_MS = 1_700_000_000_000

/** A conversation view: what a view frame says, empty unless changed. */
/**
 * The key a fake view lists `permission` under: the same for the same
 * execution and review, so a test can name the approval before the view.
 */
export function reviewKey(permission: Unkeyed<ConversationPermission>): string {
  return keyOf([permission.executionId, permission.permissionId])
}

/** 32 hex digits, the same for the same `seed`. */
function keyOf(seed: readonly string[]): string {
  let key = ""
  for (const start of [0x811c9dc5, 0x01000193, 0x2545f491, 0x9e3779b9]) {
    let hash = start
    for (const char of JSON.stringify(seed))
      hash = Math.imul(hash ^ char.charCodeAt(0), 0x01000193) >>> 0
    key += hash.toString(16).padStart(8, "0")
  }
  return key
}

/**
 * The key a fake view lists `question` under unless it names its own: the
 * same for the same execution and ask, so a test can name it before the view.
 */
export function questionKey(
  question: Pick<ConversationQuestion, "executionId" | "questionId">,
): string {
  return keyOf(["question", question.executionId, question.questionId])
}

/** How a fake view lists `question`. */
export function questionEntry(
  question: Unkeyed<ConversationQuestion>,
): ConversationQuestionEntry {
  return {
    question: question.question ?? questionKey(question),
    message: clipped(question.message, viewEntryBounds.messageBytes),
  }
}

/** All the question a fake view lists under `key` asks. */
export function shownQuestion(key: string): ConversationQuestion | undefined {
  return shownQuestions.get(key)
}

/** How a fake view lists `permission`. */
export function reviewEntry(
  permission: Unkeyed<ConversationPermission>,
): ConversationReviewEntry {
  return {
    review: reviewKey(permission),
    origin: permission.origin.kind,
    toolName: clipped(permission.toolName, viewEntryBounds.toolNameBytes),
  }
}

/**
 * The start of `text` whose JSON encoding fits `max` UTF-8 bytes, cut between
 * characters: what the gateway lists in an entry (`projection.rs`,
 * `clipped_encoded`).
 */
function clipped(text: string, max: number): string {
  const encoder = new TextEncoder()
  let end = 0
  let bytes = 0
  for (const character of text) {
    // The quotes JSON wraps one character in are not the text's.
    bytes += encoder.encode(JSON.stringify(character)).byteLength - 2
    if (bytes > max) break
    end += character.length
  }
  return text.slice(0, end)
}

// Every review a fake view was built with, by its key: what any fake
// gateway's `permissionReview` answers with while a view lists it.
const shownReviews = new Map<string, ConversationPermission>()
// And every question, which `questionReview` answers with.
const shownQuestions = new Map<string, ConversationQuestion>()

/** What the review a fake view lists under `key` shows. */
export function shownReview(key: string): ConversationPermission | undefined {
  return shownReviews.get(key)
}

/**
 * A gateway view. Its `permissions` may be what each review shows, and its
 * `questions` all each question asks: the view lists each by its key, and
 * `permissionReview` or `questionReview` answers with it.
 */
export function view(
  conversationId: string,
  {
    permissions = [],
    questions = [],
    ...change
  }: Omit<Partial<ConversationView>, "permissions" | "questions"> & {
    permissions?: readonly (ConversationReviewEntry | Unkeyed<ConversationPermission>)[]
    questions?: readonly (ConversationQuestionEntry | Unkeyed<ConversationQuestion>)[]
  } = {},
): ConversationView {
  const entries = permissions.map((listed) => {
    if (!("permissionId" in listed)) return listed
    const entry = reviewEntry(listed)
    shownReviews.set(entry.review, { ...listed, review: entry.review })
    return entry
  })
  const asked = questions.map((listed) => {
    if (!("questionId" in listed)) return listed
    const entry = questionEntry(listed)
    shownQuestions.set(entry.question, { ...listed, question: entry.question })
    return entry
  })
  return {
    permissions: entries,
    questions: asked,
    conversationId,
    revision: "r1",
    serverNowMs: GATEWAY_NOW_MS,
    messages: [],
    pending: [],
    tools: [],
    artifacts: [],
    notes: [],
    unreadable: [],
    unreadableTruncated: false,
    handoffs: [],
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
    approvalMode: "ask",
    approvalModes: [{ id: "ask", name: "Ask", description: "Ask before tools." }],
    model: "unselected",
    ...change,
  }
}

export interface FakeGateway {
  readonly client: FakeGatewayClient
  /** Every call made, in order. */
  readonly calls: Call[]
  /** Each window the client was told, newest last, with its revision; `undefined` withdrew it. */
  readonly windows: { revision: number; window: WorkspaceWindow | undefined }[]
  readonly rows: Map<string, ConversationSummary>
  readonly views: Map<string, ConversationView>
  /**
   * The execution each conversation's peek says is active, which a stop
   * names. Apart from the views on purpose: a view's message can still say
   * running after its turn stopped.
   */
  readonly active: Map<string, string>
  /**
   * What each waiting review shows, by the key its view lists it under, that
   * `permissionReview` answers with, as the review's live owner holds it.
   */
  readonly reviews: Map<string, Unkeyed<ConversationPermission>>
  /**
   * The person's channels past the default one, oldest first, each with what
   * made it, as `channel.list` answers them; `channel.create` adds to it, once
   * per request id as the gateway does.
   */
  readonly channels: {
    readonly requestId: string
    readonly params: ChannelCreateParams
    readonly channel: WireChannel
  }[]
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
  let defaultLifetime: BoatVmLifetime = { mode: "never" }
  const lifetimeOverrides = new Map<string, BoatVmLifetime>()
  const runtimeSettings = (channelId?: string): BoatRuntimeSettings => {
    const override =
      channelId === undefined ? undefined : lifetimeOverrides.get(channelId)
    return {
      ...{ defaultLifetime },
      ...(channelId === undefined ? {} : { channelId }),
      ...(override === undefined ? {} : { channelOverride: override }),
    }
  }
  const calls: FakeGateway["calls"] = []
  const windows: FakeGateway["windows"] = []
  const rows = new Map<string, ConversationSummary>()
  const views = new Map<string, ConversationView>()
  const active = new Map<string, string>()
  const reviews = new Map<string, ConversationPermission>()
  const channels: FakeGateway["channels"] = []
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
    boat: {
      runtimeSettings: async (params) => runtimeSettings(params?.channelId),
      setRuntimeSettings: async (params) => {
        const lifetime: BoatVmLifetime =
          params.mode === "after"
            ? { mode: "after", seconds: params.seconds }
            : { mode: "never" }
        if (params.channelId === undefined) defaultLifetime = lifetime
        else if (params.mode === "default") lifetimeOverrides.delete(params.channelId)
        else lifetimeOverrides.set(params.channelId, lifetime)
        return runtimeSettings(params.channelId)
      },
      status: async () => ({ state: "disconnected" as const }),
      save: async () => {
        throw new Error("Boat provisioning is unavailable in this fixture")
      },
      disconnect: async () => ({ state: "disconnected" as const }),
    },
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
      permissionReview: (...args) =>
        answer("permissionReview", args, () => {
          const [conversationId, key] = args
          const review = reviews.get(key) ?? shownReviews.get(key)
          const listed = views
            .get(conversationId)
            ?.permissions.some((entry) => entry.review === key)
          if (review === undefined || !listed)
            throw new NessaRpcError("stale_permission", "No such review is waiting")
          return { ...review, review: key }
        }) as never,
      questionReview: (...args) =>
        answer("questionReview", args, () => {
          const [conversationId, key] = args
          const question = shownQuestions.get(key)
          const listed = views
            .get(conversationId)
            ?.questions.some((entry) => entry.question === key)
          if (question === undefined || !listed)
            throw new NessaRpcError("stale_permission", "No such question is waiting")
          return question
        }) as never,
      answerQuestion: (...args) => {
        calls.push({ method: "answerQuestion", args })
        const normal = (): AnswerQuestionResult => {
          const [target] = args
          const current = views.get(target.conversationId)
          if (current)
            views.set(target.conversationId, {
              ...current,
              questions: current.questions.filter(
                (question) => question.question !== target.question,
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
      read: (id) => answer("read", [id], () => views.get(id) ?? view(id)) as never,
      setModel: (...args) =>
        answer("setModel", args, () => {
          const [id, model, options] = args
          const current = views.get(id)
          if (current)
            views.set(id, {
              ...current,
              runtime: current.runtime && { ...current.runtime, model },
            })
          return {
            requestId: options?.requestId ?? "model",
            model,
            approvalMode: current?.approvalMode ?? "ask",
            effect: "nextTurn",
          }
        }) as never,
      // As the gateway: the conversation carries on under the agent and
      // model asked for, and its view lists the switch after its last turn.
      switchAgent: (...args) =>
        answer("switchAgent", args, () => {
          const [id, agent, model, options] = args
          const requestId = options?.requestId ?? "switch-agent"
          const current = views.get(id)
          if (!current) return { requestId, applied: false }
          const last = current.messages.at(-1)?.executionId
          views.set(id, {
            ...current,
            model,
            handoffs: [
              ...current.handoffs,
              {
                handoffId: `switch-${requestId}`,
                kind: "switch",
                atMs: current.serverNowMs,
                ...(last === undefined ? {} : { afterExecutionId: last }),
                from: {
                  ...(current.runtime?.agent ? { agent: current.runtime.agent } : {}),
                  model: current.model ?? "",
                },
                to: { agent, model },
              },
            ],
            runtime: current.runtime && { ...current.runtime, agent, model },
          })
          return { requestId, applied: true }
        }) as never,
      // As the gateway: a new conversation on the source's model, whose view
      // lists the source's turns as the fork it began as, and says nothing
      // of its own yet.
      fork: (...args) =>
        answer("fork", args, () => {
          const [sourceId, options] = args
          const conversationId = options?.conversationId ?? ""
          const source = views.get(sourceId) ?? view(sourceId)
          if (!views.has(conversationId))
            views.set(conversationId, {
              ...view(conversationId, { revision: "forked" }),
              model: source.model,
              approvalMode: source.approvalMode,
              title: source.title,
              handoffs: [
                {
                  handoffId: `fork-${options?.requestId ?? conversationId}`,
                  kind: "fork",
                  atMs: source.serverNowMs,
                  turns: source.messages.map((turn) => ({
                    ...(turn.userText ? { said: turn.userText } : {}),
                    ...(turnReply(turn) ? { reply: turnReply(turn) } : {}),
                  })),
                },
              ],
            })
          const row = rows.get(sourceId)
          if (row && !rows.has(conversationId))
            rows.set(conversationId, { ...row, conversationId })
          return { conversationId }
        }) as never,
      setApprovalMode: (...args) =>
        answer("setApprovalMode", args, () => {
          const [conversationId, mode] = args
          const current = views.get(conversationId)
          if (current) views.set(conversationId, { ...current, approvalMode: mode })
          return { requestId: "set-approval-mode", mode }
        }) as never,
      setUsageLimitAutoContinue: (...args) =>
        answer("setUsageLimitAutoContinue", args, () => {
          const enabled = args[0] === true
          for (const [id, current] of views)
            views.set(id, { ...current, usageLimitAutoContinue: enabled })
          return { requestId: "usage-limit", enabled }
        }) as never,
      archive: (...args) =>
        answer("archive", args, () => {
          rows.delete(args[0])
          return { requestId: "archive", applied: true }
        }) as never,
      // As the gateway: the row and the open view take the title, and the
      // title it already had is not applied.
      rename: (...args) =>
        answer("rename", args, () => {
          const [conversationId, title] = args
          const row = rows.get(conversationId)
          if (!row || row.title === title) return { requestId: "rename", applied: false }
          rows.set(conversationId, { ...row, title })
          const view = views.get(conversationId)
          if (view) views.set(conversationId, { ...view, title })
          return { requestId: "rename", applied: true }
        }) as never,
      delete: (...args) =>
        answer("delete", args, () => {
          rows.delete(args[0])
          return { requestId: "delete", applied: true }
        }) as never,
      permissionRules: () =>
        answer("permissionRules", [], () => ({ requestId: "rules", rules: [] })) as never,
      revokePermissionRule: (...args) =>
        answer("revokePermissionRule", args, () => ({
          requestId: "revoke",
          applied: false,
        })) as never,
    },
    channels: {
      list: () =>
        answer("listChannels", [], () => ({
          channels: [defaultChannel, ...channels.map((held) => held.channel)],
          complete: true,
        })) as never,
      create: (params) =>
        answer("createChannel", [params], () => {
          const held = channels.find((entry) => entry.requestId === params.requestId)
          if (held) return held.channel
          const channelId = `ch_${"a".repeat(9)}${"abcdefghijklmnopqrstuvwxyz234567"[channels.length % 32]}`
          const channel: WireChannel = {
            channelId,
            name: params.name,
            folder: params.folder ?? `/data/channels/${channelId}`,
            folderMade: params.folder === undefined,
            agent: params.agent,
            model: params.model,
            approvalMode: params.approvalMode ?? "ask",
            coordinatorConversationId: null,
            coordinatorPlacement: params.coordinatorPlacement ?? "local",
            threadPlacement: params.threadPlacement ?? "local",
            createdAtMs: channels.length + 1,
          }
          channels.push({ requestId: params.requestId, params, channel })
          return channel
        }) as never,
    },
    workspace: {
      publish: (_windowId, revision, window) => {
        const held = windows.at(-1)?.revision ?? -1
        if (revision >= held) windows.push({ revision, window })
        return Promise.resolve({
          revision: Math.max(revision, held),
          applied: revision >= held,
        })
      },
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
              (entry) => reviews.get(entry.review)?.executionId !== executionId,
            ),
            questions: current.questions.filter(
              (asked) => shownQuestions.get(asked.question)?.executionId !== executionId,
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
    on(event: string, handler: (payload: never) => void) {
      const handlers = eventListeners.get(event) ?? new Set()
      handlers.add(handler as (payload: unknown) => void)
      eventListeners.set(event, handlers)
      return () => {
        handlers.delete(handler as (payload: unknown) => void)
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
    windows,
    rows,
    views,
    active,
    reviews,
    channels,
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
