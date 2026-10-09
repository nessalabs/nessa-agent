/**
 * The `WorkspaceSource` over the gateway (#248): the window's sessions are
 * the caller's gateway conversations, followed through `@nessa/client`'s
 * replay-to-live subscriptions (#702, `docs/design/record-subscriptions.md`).
 *
 * The orderings are tables on #248 and in that design; each row has a test in
 * `gateway-source.test.ts`. In short:
 *
 * - **Subscriptions, not polling.** While anyone listens, one list
 *   subscription follows the caller's conversations, and a view subscription
 *   follows each conversation the window opened (`transcript`), sent to
 *   (`send`), or an app called in (`appCall`, #436) — at most the gateway's
 *   published `subscriptionLimits.conversationTargets`, the least recently
 *   opened let go first (D7). Every frame is the gateway's own bounded
 *   replacement; nothing here asks again on a timer. When the list is not the
 *   whole catalogue, a frame whose conversations differ from the last one's
 *   walks `conversation.observe` (D6).
 * - **One subscription per conversation** (`follow`): an open on its way is
 *   joined, never repeated (D14), and the frames and end of any open but the
 *   current one are let go (D13).
 * - **How a subscription ends decides what follows** (`ended`): `lagging`
 *   opens again at once from the last cursor applied (D3); a refusal saying
 *   the conversation is gone takes the session out (D4); one refused for good
 *   is let go until asked again (D10); anything else is a gap, opened again
 *   on the retry clock, never at once; a lost connection waits for the
 *   connection (D5).
 * - **The retry clock** runs only while someone listens and something is not
 *   subscribed: no client, or a subscription that ended for a reason asking
 *   again may change (D17). It asks the gateway for nothing but the
 *   subscription itself.
 * - **Revisions are minted here.** The gateway's view revision is opaque and
 *   its list rows carry none, so this adapter counts: one counter per
 *   session's summary, one per conversation, each from 1, moved on when what
 *   it maps changes. A removal is the summary's next count. A view frame
 *   behind the cursor already applied in the same history is not applied
 *   (D8).
 * - **Every call settles on its own timer** (`timing.callMs`, from `clock`),
 *   connection included, rejecting `unavailable` when it runs out; and a
 *   call that has settled sends nothing consequential afterwards (`dispatch`).
 * - **Only a first message sends `create`.** The gateway resolves the
 *   conversation itself for each subscription and message — starting its
 *   agent if it is not running — so this source keeps nothing per connection
 *   but its subscriptions.
 * - **Refusals are typed.** The gateway's codes become `WorkspaceSourceError`
 *   reasons (`refusalOf`); a fault that is no answer at all is passed on as
 *   it is, for `failureReason` to log. A connection that could not be made
 *   is `signed-out`, `not-started`, `not-ready`, `not-listening`,
 *   `wrong-stage`, or `unavailable` by why (`connectFailure`, #419). A
 *   subscription that is refused is also traced (`noteReadAsked`,
 *   `noteReadRefused`).
 * - **Resync.** `{ kind: "resync" }` goes out when a connection comes back
 *   (the client reconnected, or a new one was made after the last closed —
 *   the close itself does not resync, C3), and on the first list frame after
 *   a gap: a subscription that ended or could not open, or an index that
 *   failed (S9′).
 * - **A failed connect is waited out.** For `timing.retryMs` after it,
 *   neither the subscriptions nor an MCP App connects again; a person's call
 *   connects at once (#419).
 * - **What the gateway does not keep**: pins and "always" answers are
 *   refused as `not-supported`, and so is a message asking a conversation
 *   for another model than the one the gateway says it runs (before a frame
 *   says, it cannot be told, and the message goes); there is no unread
 *   mark, so `markRead` has nothing to do. Nor does it take an initiator: it
 *   records each answer and archive as this window's authenticated caller,
 *   and cannot tell the person from an agent dispatching the same command —
 *   a gap in the record the in-memory source keeps, said here until the
 *   protocol carries it.
 * - **What it cannot see without following**: a list row says whether a
 *   conversation runs, not whether it waits on the person, so a summary says
 *   `needs-you` only for a conversation the window follows; and a question
 *   the agent asks (`view.questions`) has no place in the workspace's model
 *   and is not shown; nor is a review the gateway withholds from its view
 *   (`interactionViewError`) — the session shows no approval while it waits.
 * - **For MCP Apps (#384)**: a widget part is named in its conversation
 *   (`gatewayToolWidget`, from `transcriptFrom`); each view applied is handed
 *   to `apps.observe`, in the order applied, and a subscription the gateway
 *   refuses as `conversation_deleted` to `apps.forget`. Not a removal: a
 *   conversation missing from a complete list is archived or deleted, which
 *   a list cannot tell apart, and a closed one reopens under its id. An app's
 *   call follows its conversation (`appCall`), so the review it may wait on
 *   is shown even after the turn ended: a review an app opens changes nothing
 *   in the list row.
 *
 * It owns the client it connects (`connect`), and `dispose` closes it; an
 * app's calls go on that client too (`connected`).
 */
import {
  ConversationErrorCode,
  conversationErrorCode,
  isRetryableConnectionError,
  NessaConnectionClosedError,
  RetryableConnectError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  NessaSessionUnavailableError,
  subscriptionLimits,
  type ConnectionState,
  type ConversationApi,
  type ConversationListResult,
  type ConversationObserveCursor,
  type ConversationSummary,
  type ConversationView,
  type ConversationViewCursor,
  type Subscription,
  type SubscriptionApi,
  type SubscriptionEnd,
  type ViewFrame,
} from "@nessa/client"
import { HostRefusalError } from "../../../../host/startup-refusals"
import { isSignedOut } from "../../../../session"
import { agentForProvider } from "../../../model/composer-options"
import {
  WorkspaceSourceError,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { Transcript } from "../../model/transcript"
import type { SessionSummary, WorkspaceIndex } from "../../model/workspace-index"
import {
  gatewayChannel,
  gatewaySection,
  modelFor,
  reviewOf,
  runningModel,
  sameSummary,
  summaryFrom,
  transcriptFrom,
} from "./gateway-views"

/** What the adapter asks of a gateway client: its conversations, its subscriptions, and how its connection stands. */
export interface GatewayClient {
  readonly conversation: Pick<
    ConversationApi,
    "observe" | "create" | "send" | "answer" | "archive"
  >
  readonly subscriptions: Pick<SubscriptionApi, "view" | "list">
  readonly connectionState: ConnectionState
  onConnectionStateChange(handler: (state: ConnectionState) => void): () => void
  close(): void
}

/** The adapter's clock: the time, and a timer it can cancel. */
export interface GatewayClock {
  now(): number
  /** Runs `run` after `ms`; the returned function cancels it. */
  after(ms: number, run: () => void): () => void
}

/**
 * Who asks for the gateway's client, which decides whether it may connect
 * when none is held (`client`): a person, the subscriptions the source keeps
 * (`stream`), an MCP App, or work that uses only what is held — the
 * subscription a message begins.
 */
type Caller = "person" | "stream" | "app" | "held"

/**
 * What each caller may do when no client is held and none is connecting:
 * connect `always`, `unless-waiting` out a failed connect, or `never`. Total,
 * so a caller added later does not compile until its rule is chosen (gate 11).
 */
const callerRules: Record<
  Caller,
  { readonly connects: "always" | "unless-waiting" | "never" }
> = {
  person: { connects: "always" },
  stream: { connects: "unless-waiting" },
  app: { connects: "unless-waiting" },
  held: { connects: "never" },
}

export interface GatewayTiming {
  /** How long any call may take before it settles as `unavailable`. */
  readonly callMs: number
  /**
   * How long, after a connect failed, the subscriptions and an MCP App wait
   * before connecting again (#419); and how long a subscription that ended
   * for a reason asking again may change waits before it is opened again.
   */
  readonly retryMs: number
}

/**
 * The adapter's own budget for each call, whatever the client is doing: a
 * call of several requests (a send is `create` then `send`) shares it, and
 * one still waiting on the client when it runs out settles `unavailable`.
 */
export const defaultGatewayTiming: GatewayTiming = {
  callMs: 35_000,
  retryMs: 5_000,
}

export interface GatewaySource<
  C extends GatewayClient = GatewayClient,
> extends WorkspaceSource {
  /**
   * The client this source holds, or one connecting, within the call budget:
   * rejects `unavailable` once disposed, when none connects in time, or
   * while the source waits out a failed connect; and `signed-out` when the
   * connect it made or joined is refused that way.
   */
  connected(): Promise<C>
  /**
   * Makes one of an app's calls in conversation `conversationId`, following
   * that conversation (a conversation taken out is not followed, P9). The
   * call may wait on a review the gateway opens for it, which changes nothing
   * a list row says: without the conversation's own subscription its review
   * is not drawn (#436). The call settles as `call` does.
   */
  appCall<T>(conversationId: string, call: () => Promise<T>): Promise<T>
  /** Closes every subscription, refuses every later call, and closes the client it connected. */
  dispose(): void
}

/** Who is told each conversation view as it is applied, and each conversation deleted (#384). */
export interface GatewayViewObserver {
  /** A view, in the order its conversation's frames were applied. */
  observe(view: ConversationView): void
  /** The gateway said the conversation was deleted; its id is never used again. */
  forget(conversationId: string): void
}

/** What one conversation's latest frame left: its view, its count, and where it was read. */
interface Read {
  readonly view: ConversationView
  /** What a person could see of `view` ([`viewKey`]). */
  readonly key: string
  readonly transcript: Transcript
  readonly cursor: ConversationViewCursor
}

/**
 * What a person could see of a view: all of it but its revision. The
 * gateway's revision numbers the fold's committed content; a title, a
 * permission ask, an app's review or the agent's lifecycle change without
 * it, and a fresh fold numbers the same content anew. The gateway sends a
 * frame when this changes (`view_key`, row S9), so it is what decides here.
 */
function viewKey(view: ConversationView): string {
  return JSON.stringify({ ...view, revision: undefined })
}

/** A caller waiting for a subscription's first frame. */
interface Waiter<T> {
  resolve(value: T): void
  reject(error: unknown): void
}

/**
 * One subscription the source keeps: the open on its way or the handle it
 * gave, and who waits for its first frame. `token` names the current open:
 * frames and the end of any other are let go (D13).
 */
interface Followed<T> {
  handle: Subscription | undefined
  /**
   * Gives the current open up: closed now, or once answered. The client sends
   * the next subscribe to this target only after that close (D23).
   */
  giveUp: AbortController | undefined
  /** Whether a frame of the current open has been applied. */
  applied: boolean
  opening: Promise<void> | undefined
  /**
   * The current open's subscribe, settled once the client has answered it
   * and, when given up, closed it: what an open past the limit waits for
   * (D25). Never rejects.
   */
  subscribing: Promise<void> | undefined
  token: object
  readonly waiters: Set<Waiter<T>>
}

const followed = <T>(): Followed<T> => ({
  handle: undefined,
  giveUp: undefined,
  applied: false,
  opening: undefined,
  subscribing: undefined,
  token: {},
  waiters: new Set(),
})

/**
 * Lets the current open of `follow` go, list or conversation alike: no frame
 * or end of it applies after, nothing is on its way, and its subscription is
 * closed — now, or once answered, and before this target is subscribed again
 * (D23, D24). Who waits for a frame keeps waiting for the next open's.
 */
const letGo = <T>(follow: Followed<T>) => {
  follow.token = {}
  follow.giveUp?.abort()
  follow.giveUp = undefined
  follow.handle = undefined
  follow.opening = undefined
  follow.subscribing = undefined
  follow.applied = false
}

const settle = <T>(follow: Followed<T>, outcome: { value: T } | { error: unknown }) => {
  const waiters = [...follow.waiters]
  follow.waiters.clear()
  for (const waiter of waiters)
    if ("value" in outcome) waiter.resolve(outcome.value)
    else waiter.reject(outcome.error)
}

export function gatewaySource<C extends GatewayClient = GatewayClient>(options: {
  /** Connects a client: called again after a failed attempt, or once the last one closed. */
  readonly connect: () => Promise<C>
  readonly clock: GatewayClock
  readonly timing?: GatewayTiming
  /** Told each view applied and each conversation deleted: the window's MCP Apps. */
  readonly apps?: GatewayViewObserver
}): GatewaySource<C> {
  const { clock } = options
  const timing = options.timing ?? defaultGatewayTiming
  const listeners = new Set<(update: WorkspaceUpdate) => void>()
  let disposed = false

  // Each session's summary counter, kept after a removal so a later listing outranks it.
  const summaryCounts = new Map<string, number>()
  const summaries = new Map<string, SessionSummary>()
  // Sessions this source has taken out — archived here or elsewhere, or
  // missing from a complete list — until a list names them again: the one
  // fact `takenOut` reads (R8).
  const removedIds = new Set<string>()
  const takenOut = (sessionId: string) => removedIds.has(sessionId)
  const rows = new Map<string, ConversationSummary>()
  // Each conversation's counter, and its latest frame.
  const transcriptCounts = new Map<string, number>()
  const reads = new Map<string, Read>()
  // When each message was first seen, per session: the gateway's view has no times.
  const firstSeen = new Map<string, Map<string, number>>()

  // Nothing is said after `dispose`, which lets every listener go and admits no new one.
  const emit = (update: WorkspaceUpdate) => {
    // A listener's fault is its own: it neither stops the others nor the source.
    for (const listener of [...listeners]) {
      try {
        listener(update)
      } catch (error) {
        console.error("A workspace listener failed", error)
      }
    }
  }

  // The apps' fault is theirs: it neither stops the frame nor the source.
  const tellApps = (tell: () => void) => {
    try {
      tell()
    } catch (error) {
      console.error("The window's MCP Apps failed to take a conversation view", error)
    }
  }

  // A subscription that ended or could not open, or an index that failed,
  // since the last resync: the next list frame says resync.
  let gap = false
  const resync = () => {
    gap = false
    emit({ kind: "resync" })
  }

  /** A read trace, unless the source was disposed: that answer says nothing (S13). */
  const noteTraced = (
    trace: ReadTrace | undefined,
    refused: unknown,
    cause?: unknown,
  ) => {
    if (!trace || disposed) return
    noteReadRefused(trace.subject, trace.sessionId, refused, cause ?? refused)
  }

  /**
   * Settles `work` within the call budget, rejecting `unavailable` when it
   * runs out. The work may go on after its caller was answered; it is told
   * whether its call is still open (`live`), and sends nothing consequential
   * once it is not (`dispatch`, C6). A `trace` records the ask and, when the
   * call is refused, the reason — on this same turn, not a later one.
   */
  const within = <T>(
    work: (live: () => boolean) => Promise<T>,
    trace?: ReadTrace,
  ): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      if (disposed) return reject(new WorkspaceSourceError("unavailable"))
      let settled = false
      const cancel = clock.after(timing.callMs, () => {
        const already = settled
        settled = true
        // The budget ran out: the same refusal the caller is about to get.
        if (!already) noteTraced(trace, new WorkspaceSourceError("unavailable"))
        reject(new WorkspaceSourceError("unavailable"))
      })
      const live = () => !settled && !disposed
      Promise.resolve()
        .then(() => {
          if (!disposed) noteReadAsked(trace)
          return work(live)
        })
        .then(
          (value) => {
            settled = true
            cancel()
            if (disposed) reject(new WorkspaceSourceError("unavailable"))
            else resolve(value)
          },
          (error: unknown) => {
            const already = settled
            settled = true
            cancel()
            const refused = refusalOf(error)
            if (!already) noteTraced(trace, refused, error)
            reject(refused)
          },
        )
    })

  // The connection: one client at a time, connected on first need.
  let current: { client: C; off: () => void } | undefined
  let connecting: Promise<C> | undefined
  let connectedBefore = false
  // After a connect that failed — refused, or out of time — nothing in the
  // background connects again until `waitUntil` (#419): each connect asks
  // the host for the gateway and its credential, and the gateway to
  // authenticate, so a window that cannot connect must not ask again at once.
  // A person's call connects at once, and if it fails the wait starts again;
  // a connect that succeeds ends it. Who may connect is `client`'s alone to
  // decide.
  let waitUntil = 0
  const connectFailed = () => {
    waitUntil = clock.now() + timing.retryMs
  }
  /** Takes a client that connected as the current one. */
  const adopt = (connected: C) => {
    const off = connected.onConnectionStateChange((state) => {
      if (current?.client !== connected) return
      if (state.status === "connected") {
        // Back: every subscription ended `disconnected` with the connection.
        restore()
        resync()
      } else if (state.status === "closed") {
        // Gone for good: the next call connects again. That next client
        // resyncs when one had connected before (C3). The close is not a
        // gap. A list frame already applied stays.
        current.off()
        current = undefined
        scheduleRetry()
      }
    })
    current = { client: connected, off }
    waitUntil = 0
    // A connection after another is a reconnect: what it missed is read again.
    if (connectedBefore) {
      restore()
      resync()
    }
    connectedBefore = true
  }
  /**
   * The current client, or one connecting for every caller at once — and,
   * when there is neither, whether to connect, by who is asking: the one
   * place that is decided (#419).
   *
   * - `person`: a foreground call — the index (Try Again, or the store's
   *   own re-read on a resync), opening a session, a message, an answer, an
   *   archive — connects at once.
   * - `stream`: the subscriptions this source keeps connect unless the
   *   source is waiting out a failed connect.
   * - `app`: an MCP App connects unless the source is waiting.
   * - `held`: the subscription a message begins uses the client held and
   *   never connects in its place.
   *
   * Any of them joins a connect already on its way. An attempt has the call
   * budget too: one that outlasts it is given up — and counts as a failed
   * connect — and a client it brings late is closed unused.
   */
  const client = (who: Caller): Promise<C> => {
    if (disposed) return Promise.reject(new WorkspaceSourceError("unavailable"))
    if (current) return Promise.resolve(current.client)
    if (connecting) return connecting
    const rule = callerRules[who]
    if (rule.connects === "never")
      return Promise.reject(new WorkspaceSourceError("unavailable"))
    if (rule.connects === "unless-waiting" && clock.now() < waitUntil)
      return Promise.reject(new WorkspaceSourceError("unavailable"))
    const attempt = new Promise<C>((resolve, reject) => {
      let over = false
      const giveUp = (
        failure: WorkspaceSourceError = new WorkspaceSourceError("unavailable"),
      ) => {
        over = true
        connectFailed()
        reject(failure)
      }
      const cancel = clock.after(timing.callMs, () => giveUp())
      options.connect().then(
        (connected) => {
          cancel()
          if (over || disposed) {
            connected.close()
            if (!over) giveUp()
            return
          }
          over = true
          adopt(connected)
          resolve(connected)
        },
        (error: unknown) => {
          cancel()
          if (over) return
          // Disposed meanwhile: the source answers nothing but `unavailable`,
          // and has nothing left to say about why (S13).
          if (disposed) return giveUp()
          // Not reaching the gateway is an answer the window can show; why is logged.
          console.warn("Could not connect to the gateway", error)
          giveUp(connectFailure(error))
        },
      )
    })
    connecting = attempt
    const done = () => {
      if (connecting === attempt) connecting = undefined
    }
    attempt.then(done, done)
    return attempt
  }

  /**
   * Sends one consequential request — an answer, an archive, a create, a
   * message — on the current client, only while the call that asked for it
   * is still open: one whose caller was told `unavailable` sends nothing
   * afterwards (C6). What guards the request runs in `request`, right
   * before it, never earlier.
   */
  const dispatch = async <T>(
    live: () => boolean,
    request: (connected: C) => Promise<T>,
  ): Promise<T> => {
    const connected = await client("person")
    if (!live()) throw new WorkspaceSourceError("unavailable")
    return request(connected)
  }

  // List frames, their observe walks, and archives run one at a time.
  let listing: Promise<unknown> = Promise.resolve()
  const inTurn = <T>(after: Promise<unknown>, work: () => Promise<T>) => {
    const turn = after.then(work, work)
    return { turn, settled: turn.then(noop, noop) }
  }

  /** The model a session is known to run on: the gateway's own word for it, from its last frame. */
  const knownModel = (sessionId: string) => runningModel(reads.get(sessionId)?.view)
  const modelOf = (sessionId: string) => modelFor(knownModel(sessionId))

  /**
   * Says a session's summary again from everything known of it, at its next
   * count when it says something new, or was taken out. Nothing without a row.
   */
  const publish = (sessionId: string): void => {
    const row = rows.get(sessionId)
    if (!row) return
    const said = summaryFrom(row, {
      model: modelOf(sessionId),
      waitingOnPerson: Boolean(reads.get(sessionId)?.transcript.approval),
    })
    const held = summaries.get(sessionId)
    if (held && !takenOut(sessionId) && sameSummary(held, said)) return
    const revision = (summaryCounts.get(sessionId) ?? 0) + 1
    summaryCounts.set(sessionId, revision)
    const summary: SessionSummary = { ...said, revision }
    summaries.set(sessionId, summary)
    removedIds.delete(sessionId)
    emit({ kind: "session", session: summary })
  }

  // The conversations followed, least recently opened first (D7).
  const views = new Map<string, Followed<Transcript>>()
  const list: Followed<void> = followed()
  // Sessions whose archive is on its way: a list frame that arrives meanwhile
  // may have been read before it, and does not list them again (W6).
  const archiving = new Set<string>()
  // The subscribes of conversations let go past the limit while on their
  // way: the gateway counts one against the limit until it is answered and
  // closed, so the next open waits for them (D25).
  let evicted: Promise<void> | undefined
  // How many list frames have come: only the newest queued is applied (D18).
  let listFrames = 0

  /** Lets a conversation's subscription go: no frame or end of it applies after. */
  const unfollow = (sessionId: string, why: unknown) => {
    const follow = views.get(sessionId)
    if (!follow) return
    views.delete(sessionId)
    letGo(follow)
    settle(follow, { error: why })
  }

  /** Takes a session out, at its summary's next count; one already out stays as it is. */
  const remove = (sessionId: string): void => {
    if (takenOut(sessionId)) return
    const revision = (summaryCounts.get(sessionId) ?? 0) + 1
    summaryCounts.set(sessionId, revision)
    // Taken out whether or not a list had named it yet: an archive of a
    // session just begun here is remembered too.
    removedIds.add(sessionId)
    rows.delete(sessionId)
    unfollow(sessionId, new WorkspaceSourceError("unknown-session"))
    // Its last frame goes with it: listed again, nothing it said then speaks for it.
    reads.delete(sessionId)
    emit({ kind: "session-removed", sessionId, revision })
  }

  /**
   * Applies a list or a finished observation pass: each row said again, and —
   * only when `complete` is true — a session it does not name taken out. An
   * incomplete result proves nothing of what it leaves out.
   */
  const applyList = (result: ConversationListResult): void => {
    const listed = new Set<string>()
    for (const row of result.conversations) {
      listed.add(row.conversationId)
      rows.set(row.conversationId, row)
      publish(row.conversationId)
    }
    if (result.complete)
      for (const sessionId of summaries.keys())
        if (!takenOut(sessionId) && !listed.has(sessionId)) remove(sessionId)
  }

  /**
   * Walks `conversation.observe` after an incomplete list. A page that finishes
   * the pass replaces the list as membership. A page that cannot resume —
   * no cursor, or a cursor that is not strictly later in creation order —
   * keeps every row seen so far and does not claim the catalogue is exhausted.
   */
  const observedCatalogue = async (
    connected: GatewayClient,
    listed: ConversationListResult,
    live: () => boolean,
    caller: () => boolean,
  ): Promise<ConversationListResult> => {
    const observed = new Map<string, ConversationSummary>()
    let cursor: ConversationObserveCursor | undefined
    for (;;) {
      if (!live() || !caller()) throw new WorkspaceSourceError("unavailable")
      const page = await connected.conversation.observe(
        cursor === undefined ? {} : { cursor },
      )
      for (const row of page.conversations) observed.set(row.conversationId, row)
      if (page.complete) return { conversations: [...observed.values()], complete: true }
      if (page.cursor === undefined || !cursorAdvances(cursor, page.cursor)) {
        const merged = new Map(
          listed.conversations.map((row) => [row.conversationId, row] as const),
        )
        for (const [id, row] of observed) merged.set(id, row)
        return { conversations: [...merged.values()], complete: false }
      }
      cursor = page.cursor
    }
  }

  /**
   * One list frame, in the list's turn. A complete one is the membership. An
   * incomplete one walks `conversation.observe` in the same turn, until the
   * pass finishes or a page cannot resume (`docs/design/ui-workspace-load.md`):
   * the gateway sends one when a row it left out changed, though the rows it
   * carries are the same, and only a walk sees that row (D18). A frame of
   * a subscription no longer current, or one whose walk outlived the
   * subscription, applies nothing.
   */
  const listFrame = (
    connected: GatewayClient,
    result: ConversationListResult,
    current: () => boolean,
  ) => {
    const crossed = new Set(archiving)
    const mine = ++listFrames
    const { settled } = inTurn(listing, async () => {
      // A frame is a replacement: one queued behind a walk gives way to the
      // newest, so frames that come during a walk cost one more walk (D18).
      if (!current() || mine !== listFrames) return
      let applied = result
      if (!result.complete) {
        try {
          applied = await within(
            (live) => observedCatalogue(connected, result, live, current),
            { subject: "index" },
          )
        } catch {
          // The rows the frame named still apply; the rest waits for the next frame.
          if (!current()) return
          gap = true
        }
      }
      if (!current()) return
      applyList({
        ...applied,
        conversations: applied.conversations.filter(
          (row) => !(crossed.has(row.conversationId) && takenOut(row.conversationId)),
        ),
      })
      list.applied = true
      if (gap) resync()
      settle(list, { value: undefined })
    })
    listing = settled
  }

  /** The source's reason for a subscription's end, as a refusal of the call that waited on it. */
  const endRefusal = (end: SubscriptionEnd): WorkspaceSourceError => {
    if (end.reason !== "refused" || end.code === undefined)
      return new WorkspaceSourceError(
        end.reason === "too_large" || end.reason === "invalid_frame"
          ? "not-supported"
          : "unavailable",
      )
    const code = conversationErrorCode(end.code)
    return new WorkspaceSourceError(
      code === undefined ? "unavailable" : reasonFor(code, false),
    )
  }

  /** Opens the list subscription, unless it is open or opening. */
  const openList = (who: Caller): Promise<void> => {
    if (list.handle) return Promise.resolve()
    if (list.opening) return list.opening
    const token = {}
    list.token = token
    list.applied = false
    const { signal } = (list.giveUp = new AbortController())
    const current = () => list.token === token && !disposed
    const opening = within(
      async (live) => {
        const connected = await client(who)
        if (!live() || !current()) throw new WorkspaceSourceError("unavailable")
        const handle = await connected.subscriptions
          .list(
            {
              list: (result) => {
                if (current()) listFrame(connected, result, current)
              },
              ended: (end) => {
                if (current()) listEnded(end)
              },
            },
            { signal },
          )
          .catch((error: unknown) => {
            // Given up on its way: the answer of an open let go once answered.
            if (signal.aborted) throw new WorkspaceSourceError("unavailable")
            throw error
          })
        if (!live() || !current()) {
          void handle.close()
          throw new WorkspaceSourceError("unavailable")
        }
        list.handle = handle
      },
      { subject: "index" },
    )
    list.opening = opening
    opening.then(
      () => {
        if (list.opening === opening) list.opening = undefined
      },
      (error: unknown) => {
        if (list.opening === opening) list.opening = undefined
        if (list.token !== token) return
        // Let go: anything it brings later is not this list's.
        list.token = {}
        gap = true
        settle(list, { error })
        scheduleRetry()
      },
    )
    return opening
  }

  /** How the list subscription ended decides what follows (D9). */
  const listEnded = (end: SubscriptionEnd) => {
    // Ended, even before its open finished: that open is let go.
    list.handle = undefined
    list.opening = undefined
    list.applied = false
    list.token = {}
    if (end.reason === "disconnected") return
    if (end.reason === "lagging") {
      void openList("stream").catch(noop)
      return
    }
    gap = true
    settle(list, { error: endRefusal(end) })
    scheduleRetry()
  }

  /** The index once a list frame is applied: opening the list subscription as `who`. */
  const listReady = async (who: Caller): Promise<void> => {
    if (list.applied) return
    const ready = new Promise<void>((resolve, reject) =>
      list.waiters.add({ resolve, reject }),
    )
    // An open that fails rejects both; the open's rejection is the one answered.
    ready.catch(noop)
    await openList(who)
    await ready
  }

  const seenIn = (sessionId: string) => {
    const seen = firstSeen.get(sessionId) ?? new Map<string, number>()
    firstSeen.set(sessionId, seen)
    return (messageId: string) => {
      const at = seen.get(messageId)
      if (at !== undefined) return at
      const now = clock.now()
      seen.set(messageId, now)
      return now
    }
  }

  /**
   * Applies a frame: a view a person would see differently is the
   * conversation's next count, and is said; one that differs at most in its
   * revision is the same transcript (R6, D2).
   */
  const applyRead = (
    sessionId: string,
    view: ConversationView,
    cursor: ConversationViewCursor,
  ): Transcript => {
    const held = reads.get(sessionId)
    const key = viewKey(view)
    if (held && held.key === key) {
      reads.set(sessionId, { ...held, cursor })
      return held.transcript
    }
    const revision = (transcriptCounts.get(sessionId) ?? 0) + 1
    transcriptCounts.set(sessionId, revision)
    const transcript = transcriptFrom(view, revision, seenIn(sessionId))
    reads.set(sessionId, { view, key, transcript, cursor })
    tellApps(() => options.apps?.observe(view))
    emit({ kind: "transcript", transcript })
    // The summary follows what the frame says: an approval waiting, the model it runs on.
    publish(sessionId)
    return transcript
  }

  /** A view frame of the current subscription: applied unless behind the cursor applied (D8). */
  const viewFrame = (
    sessionId: string,
    follow: Followed<Transcript>,
    frame: ViewFrame,
  ) => {
    const held = reads.get(sessionId)
    if (held && behind(frame.cursor, held.cursor)) return
    const transcript = applyRead(sessionId, frame.view, frame.cursor)
    follow.applied = true
    settle(follow, { value: transcript })
  }

  /**
   * The port's rule for a session this source has taken out — archived here
   * or elsewhere, or missing from a complete list: it holds no such session,
   * follows none and begins none under its id (`failure.ts`, R8).
   */
  const held = (sessionId: string) => {
    if (takenOut(sessionId)) throw new WorkspaceSourceError("unknown-session")
  }

  /**
   * A refusal of a conversation's subscription: gone takes the session out
   * (and a deletion its apps' calls with it); refused for good lets the
   * subscription go until asked again (R11, D10); anything else is a gap, and
   * the subscription is opened again on the retry clock.
   */
  const refused = (
    sessionId: string,
    follow: Followed<Transcript>,
    refusal: unknown,
    deleted: boolean,
  ) => {
    if (gone(refusal)) {
      remove(sessionId)
      if (deleted) tellApps(() => options.apps?.forget(sessionId))
      settle(follow, { error: refusal })
      return
    }
    if (refusedForGood(refusal)) {
      console.warn("[nessa] conversation subscription let go", { sessionId })
      unfollow(sessionId, refusal)
      return
    }
    gap = true
    settle(follow, { error: refusal })
    scheduleRetry()
  }

  /** How a conversation's subscription ended decides what follows (D3, D4, D5, D10). */
  const viewEnded = (
    sessionId: string,
    follow: Followed<Transcript>,
    end: SubscriptionEnd,
  ) => {
    // Ended, even before its open finished: that open is let go.
    follow.handle = undefined
    follow.opening = undefined
    follow.subscribing = undefined
    follow.applied = false
    follow.token = {}
    if (end.reason === "disconnected") return
    if (end.reason === "lagging") {
      open(sessionId, follow, "stream")
      return
    }
    refused(
      sessionId,
      follow,
      endRefusal(end),
      end.reason === "refused" && end.code === ConversationErrorCode.ConversationDeleted,
    )
  }

  /**
   * Opens `sessionId`'s subscription from the cursor last applied, unless one
   * is open or on its way: the one place it is opened (D14). A cursor ahead
   * of the history the gateway holds opens once more without it.
   */
  const open = (sessionId: string, follow: Followed<Transcript>, who: Caller) => {
    if (follow.handle || follow.opening) return
    const token = {}
    follow.token = token
    follow.applied = false
    const { signal } = (follow.giveUp = new AbortController())
    const current = () =>
      follow.token === token && views.get(sessionId) === follow && !disposed
    let deleted = false
    const subscribe = async (connected: C, after: ConversationViewCursor | undefined) =>
      connected.subscriptions.view(
        sessionId,
        {
          view: (frame) => {
            if (current()) viewFrame(sessionId, follow, frame)
          },
          ended: (end) => {
            if (current()) viewEnded(sessionId, follow, end)
          },
        },
        after === undefined ? { signal } : { after, signal },
      )
    const opening = within(
      async (live) => {
        const connected = await client(who)
        if (!live() || !current()) throw new WorkspaceSourceError("unavailable")
        if (evicted) {
          await evicted
          if (!live() || !current()) throw new WorkspaceSourceError("unavailable")
        }
        let handle: Subscription
        try {
          const subscribing = subscribe(connected, reads.get(sessionId)?.cursor).catch(
            (error: unknown) => {
              if (!(error instanceof NessaRpcError) || error.code !== "cursor_ahead")
                throw error
              // The client is ahead of the history the gateway holds: start over.
              reads.delete(sessionId)
              if (!live() || !current()) throw new WorkspaceSourceError("unavailable")
              return subscribe(connected, undefined)
            },
          )
          follow.subscribing = subscribing.then(noop, noop)
          handle = await subscribing
        } catch (error) {
          // Given up on its way: the answer of an open let go once answered,
          // as for the list.
          if (signal.aborted) throw new WorkspaceSourceError("unavailable")
          deleted = deletedConversation(error)
          throw error
        }
        if (!live() || !current()) {
          void handle.close().catch(noop)
          throw new WorkspaceSourceError("unavailable")
        }
        follow.handle = handle
      },
      { subject: "conversation", sessionId },
    )
    follow.opening = opening
    opening.then(
      () => {
        if (follow.opening === opening) follow.opening = undefined
      },
      (error: unknown) => {
        if (follow.opening === opening) follow.opening = undefined
        if (follow.token !== token || views.get(sessionId) !== follow) return
        // Let go: anything it brings later is not this follow's.
        follow.token = {}
        refused(sessionId, follow, error, deleted)
      },
    )
  }

  /**
   * Follows `sessionId` as `who`: the least recently opened conversation is
   * let go past the published limit (D7), and an open already on its way is
   * joined (D14).
   */
  const follow = (sessionId: string, who: Caller): Followed<Transcript> => {
    let follow = views.get(sessionId)
    if (follow) views.delete(sessionId)
    else {
      follow = followed<Transcript>()
      while (views.size >= subscriptionLimits.conversationTargets) {
        const [oldest] = views.keys()
        // Answered and closed before this one is subscribed (D25); one
        // already answered is closed at once, ahead of the next subscribe.
        const leaving = views.get(oldest)
        if (leaving?.opening && leaving.subscribing) {
          const waiting = Promise.all([evicted, leaving.subscribing]).then(noop)
          evicted = waiting
          void waiting.then(() => {
            if (evicted === waiting) evicted = undefined
          })
        }
        unfollow(oldest, new WorkspaceSourceError("unavailable"))
        // Not followed, it says only what its row says (`needs-you` is a frame's).
        reads.delete(oldest)
        publish(oldest)
      }
    }
    views.set(sessionId, follow)
    open(sessionId, follow, who)
    return follow
  }

  /** The conversation as its subscription last said it, waiting for the first frame when none is held (D20). */
  const transcriptOf = (sessionId: string, who: Caller): Promise<Transcript> => {
    held(sessionId)
    const followedNow = follow(sessionId, who)
    const last = reads.get(sessionId)
    if (followedNow.applied && last) return Promise.resolve(last.transcript)
    return new Promise<Transcript>((resolve, reject) =>
      followedNow.waiters.add({ resolve, reject }),
    )
  }

  // The retry clock: one timer, only while someone listens and something is
  // not subscribed (D17).
  let cancelRetry: (() => void) | undefined
  const scheduleRetry = () => {
    if (disposed || listeners.size === 0 || cancelRetry) return
    const wait = Math.max(timing.retryMs, waitUntil - clock.now())
    cancelRetry = clock.after(wait, () => {
      cancelRetry = undefined
      restore()
    })
  }
  /** Opens every subscription this source keeps that is not open: the list, and each conversation followed. */
  const restore = () => {
    if (disposed || listeners.size === 0) return
    void openList("stream").catch(noop)
    for (const [sessionId, follow] of views) open(sessionId, follow, "stream")
  }
  /** Closes every subscription; the conversations followed are opened again from their cursors (`restore`). */
  const stopStreams = () => {
    cancelRetry?.()
    cancelRetry = undefined
    // An open on its way is let go too, so a listener back before it is
    // answered opens anew, after its close (D24).
    letGo(list)
    for (const follow of views.values()) letGo(follow)
  }

  const waitingReview = async (sessionId: string, approvalId: string) => {
    const review = reviewOf(approvalId)
    if (!review) throw new WorkspaceSourceError("not-waiting")
    await transcriptOf(sessionId, "person")
    const permission = reads
      .get(sessionId)
      ?.view.permissions.find(
        (asked) =>
          asked.executionId === review.executionId &&
          asked.permissionId === review.permissionId,
      )
    if (!permission) throw new WorkspaceSourceError("not-waiting")
    return permission
  }

  /**
   * Answers a review with `optionId` when that option decides `effect`.
   * Another option of the same effect is a different answer. One the review
   * does not offer, or that decides the other way, is not supported. The
   * conversation after the answer follows as a frame (D12).
   */
  const answer = (
    sessionId: string,
    approvalId: string,
    optionId: string,
    effect: "allow" | "deny",
  ) =>
    within(async (live) => {
      const permission = await waitingReview(sessionId, approvalId)
      const option = permission.options.find((offered) => offered.id === optionId)
      if (!option || option.effect !== effect)
        throw new WorkspaceSourceError("not-supported")
      await dispatch(live, (connected) =>
        connected.conversation.answer(
          sessionId,
          permission.executionId,
          permission.permissionId,
          option.id,
        ),
      )
    })

  return {
    index: () =>
      within(async () => {
        await listReady("person").catch((error: unknown) => {
          gap = true
          throw error
        })
        // The index is the resync: what any gap missed is read now (S20).
        gap = false
        const index: WorkspaceIndex = {
          sections: [gatewaySection],
          channels: [gatewayChannel],
          sessions: [...summaries.values()].filter((summary) => !takenOut(summary.id)),
        }
        return index
      }),
    // A transcript sends no `create`. The session is followed from now on —
    // a subscription that fails is opened again on the retry clock — unless
    // it is gone or refused for good (R3, R8, R11).
    transcript: (sessionId) => within(() => transcriptOf(sessionId, "person")),
    subscribe(listener) {
      if (disposed) return noop
      listeners.add(listener)
      restore()
      return () => {
        listeners.delete(listener)
        if (listeners.size === 0) stopStreams()
      }
    },
    send: (message) =>
      within(async (live) => {
        // Only a first message opens its conversation, on the model chosen for it.
        if (message.start) {
          const agent = agentForProvider(message.model.provider)
          await dispatch(live, (connected) => {
            held(message.sessionId)
            return connected.conversation.create({
              conversationId: message.sessionId,
              ...(agent ? { agent } : {}),
              model: message.model.modelId,
            })
          })
        }
        await dispatch(live, (connected) => {
          held(message.sessionId)
          // The gateway keeps a conversation on the model it was created with:
          // a message asking for another than the one it says it runs is
          // refused rather than sent on the old one, checked here, at the
          // send (W8). Not said yet, it cannot be told.
          const known = knownModel(message.sessionId)
          if (
            known &&
            (known.provider !== message.model.provider ||
              known.modelId !== message.model.modelId)
          )
            throw new WorkspaceSourceError("not-supported")
          try {
            return connected.conversation.send(message.sessionId, message.text, [], [], {
              // The message's id names the same turn however often it is sent.
              executionId: message.messageId,
              requestId: message.messageId,
            })
          } catch (error) {
            // The client refuses a message past its bounds before sending it,
            // by throwing `TypeError` at once (`ConversationApi.send`): sent
            // again, it is refused again (F5).
            if (error instanceof TypeError)
              throw new WorkspaceSourceError("not-supported")
            throw error
          }
        })
        // Taken out while it was on its way: sent, but not followed (R8).
        if (!takenOut(message.sessionId)) follow(message.sessionId, "held")
        publish(message.sessionId)
      }),
    approve: (sessionId, approvalId, scope, _initiator, optionId) =>
      scope === "always"
        ? // No option the gateway shows reaches past its request: the projection
          // offers no review with one (nessa-server's projection test
          // `a_review_reaching_beyond_its_request_is_not_offered`).
          Promise.reject(new WorkspaceSourceError("not-supported"))
        : answer(sessionId, approvalId, optionId, "allow"),
    deny: (sessionId, approvalId, _initiator, optionId) =>
      answer(sessionId, approvalId, optionId, "deny"),
    // The gateway keeps no pin.
    setPinned: () => Promise.reject(new WorkspaceSourceError("not-supported")),
    archive: (sessionId) =>
      within(async (live) => {
        // In the list's turn, so no list frame before it can list the session
        // again; and one arriving while it is on its way does not either (W6).
        archiving.add(sessionId)
        const { turn, settled } = inTurn(listing, async () => {
          // Taken out already, by this window or a list: refused, and asked of nobody (W6b).
          held(sessionId)
          // Its own timer, so one that never answers does not hold the lists
          // behind it (C5); and sent only while the archive's call is open (C6).
          await within(() =>
            dispatch(live, (connected) => connected.conversation.archive(sessionId)),
          )
          remove(sessionId)
        })
        listing = settled
        void settled.then(() => archiving.delete(sessionId))
        await turn
      }),
    // The gateway keeps no unread mark: every summary is read already.
    markRead: () => within(() => Promise.resolve()),
    // An app calls on its own schedule, not a person's: in the background.
    connected: () => within(() => client("app")),
    appCall(conversationId, call) {
      // Followed, so the review the call may open is shown (#436); not one taken out (P9).
      if (!takenOut(conversationId)) follow(conversationId, "app")
      return Promise.resolve().then(call)
    },
    dispose() {
      disposed = true
      stopStreams()
      settle(list, { error: new WorkspaceSourceError("unavailable") })
      for (const sessionId of [...views.keys()])
        unfollow(sessionId, new WorkspaceSourceError("unavailable"))
      listeners.clear()
      current?.off()
      current?.client.close()
      current = undefined
    },
  }
}

function noop() {}

/** Whether `cursor` is behind `applied` in the same stored history (D8). Another history replaces. */
function behind(
  cursor: ConversationViewCursor,
  applied: ConversationViewCursor,
): boolean {
  if (cursor.incarnation !== applied.incarnation) return false
  try {
    return BigInt(cursor.position) < BigInt(applied.position)
  } catch {
    return false
  }
}

/** Whether a failure says the session is gone: no such conversation, or one taken out. */
function gone(error: unknown): boolean {
  return error instanceof WorkspaceSourceError && error.reason === "unknown-session"
}

/** Whether the gateway answered that a conversation was deleted, which it never reuses the id of. */
function deletedConversation(error: unknown): boolean {
  const code =
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError
      ? error.code
      : error instanceof NessaRpcError
        ? conversationErrorCode(error.code)
        : undefined
  return code === ConversationErrorCode.ConversationDeleted
}

/**
 * Why no client connected, by the error's type.
 *
 * A host refusal is `HostRefusalError` (the command's typed payload). A
 * socket that never opened is `RetryableConnectError`, or a close `1006`
 * on this error itself — a health probe that wraps one stays whatever the
 * probe is, so a later RPC timeout is not rewritten as "not answering".
 * `signed-out` is the gateway refusing the credential (`isSignedOut`).
 * Anything else, including a host sentence this code does not parse, is
 * `unavailable`.
 */
function connectFailure(error: unknown): WorkspaceSourceError {
  if (error instanceof HostRefusalError) {
    if (error.reason === "not-provisioned") return new WorkspaceSourceError("not-started")
    if (error.reason === "not-ready") return new WorkspaceSourceError("not-ready")
    if (error.reason === "wrong-stage" && error.bundle && error.requested) {
      return new WorkspaceSourceError("wrong-stage", {
        bundle: error.bundle,
        requested: error.requested,
      })
    }
  }
  if (
    error instanceof RetryableConnectError ||
    (error instanceof NessaConnectionClosedError && error.code === 1006)
  )
    return new WorkspaceSourceError("not-listening")
  if (isSignedOut(error)) return new WorkspaceSourceError("signed-out")
  return new WorkspaceSourceError("unavailable")
}

/** Whether a failure is a refusal that asking again changes nothing of (`reasonFor`). */
function refusedForGood(error: unknown): boolean {
  return error instanceof WorkspaceSourceError && error.reason === "not-supported"
}

/**
 * Which subscription the desktop opened. `index` is the list subscription,
 * `conversation.subscribeList`; an incomplete list frame's walk of
 * `conversation.observe` is traced under it. `conversation` is one
 * conversation's `conversation.subscribe`.
 */
type ReadTrace = { subject: "index" | "conversation"; sessionId?: string }

/**
 * The desktop opened a subscription. A debug line: the dev console forwards
 * warnings, not this. No trace means this call opens none.
 */
function noteReadAsked(trace: ReadTrace | undefined): void {
  if (!trace) return
  try {
    console.debug(
      "[nessa] conversation read asked",
      readTrace(trace.subject, trace.sessionId),
    )
  } catch {
    // A diagnostic must not change the read.
  }
}

/**
 * The read was refused. A warning, so the dev console's existing forward
 * keeps it: the subject, the session, the workspace reason, and a gateway or
 * socket hint already on the error. Never the error's message.
 */
function noteReadRefused(
  subject: "index" | "conversation",
  sessionId: string | undefined,
  refused: unknown,
  cause: unknown,
): void {
  try {
    console.warn("[nessa] conversation read refused", {
      ...readTrace(subject, sessionId),
      reason: refused instanceof WorkspaceSourceError ? refused.reason : "unavailable",
      ...readHint(cause),
    })
  } catch {
    // A diagnostic must not change the refusal.
  }
}

/**
 * Whether `next` is strictly later than `previous` in one observation pass.
 * Creation revisions compare as integers. `"10"` is after `"9"`.
 */
function cursorAdvances(
  previous: ConversationObserveCursor | undefined,
  next: ConversationObserveCursor,
): boolean {
  if (previous === undefined) return true
  if (next.incarnation !== previous.incarnation || next.boundary !== previous.boundary)
    return false
  let creation: bigint
  let earlier: bigint
  try {
    creation = BigInt(next.creation)
    earlier = BigInt(previous.creation)
  } catch {
    return false
  }
  if (creation > earlier) return true
  if (creation < earlier) return false
  return next.id > previous.id
}

function readTrace(subject: "index" | "conversation", sessionId?: string) {
  return {
    subject,
    method: subject === "index" ? "conversation.subscribeList" : "conversation.subscribe",
    ...(sessionId === undefined ? {} : { sessionId }),
  }
}

/** The gateway or socket fact already on the error, and nothing it says in prose. */
function readHint(error: unknown): {
  code?: string
  closeReason?: string
  socket?: string
} {
  if (error instanceof NessaRpcError) return { code: error.code }
  if (
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError
  )
    return error.code === undefined ? {} : { code: error.code }
  if (error instanceof NessaConnectionClosedError)
    return {
      code: String(error.code),
      closeReason: error.closeReason,
      socket: "closed",
    }
  if (error instanceof NessaSessionUnavailableError) return { socket: "unavailable" }
  if (isRetryableConnectionError(error)) return { socket: "retryable" }
  return {}
}

/**
 * The source's reason for a failed call, by the error's type. Whether a
 * refused command may have taken effect is the client's to say (`uncertain`,
 * from `rejectedBeforeDispatch`): one that may have is `unavailable`, sent
 * again under its id. Otherwise the gateway's code says why (`reasonFor`);
 * no answer at all is `unavailable`; and anything else — a fault, such as an
 * answer the client could not read — is passed on as it is.
 */
export function refusalOf(error: unknown): unknown {
  if (error instanceof WorkspaceSourceError) return error
  if (
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError
  )
    return new WorkspaceSourceError(
      error.code === undefined ? "unavailable" : reasonFor(error.code, error.uncertain),
    )
  if (error instanceof NessaRpcError) {
    const code = conversationErrorCode(error.code)
    return new WorkspaceSourceError(
      code === undefined ? "unavailable" : reasonFor(code, false),
    )
  }
  if (
    error instanceof NessaConnectionClosedError ||
    error instanceof NessaSessionUnavailableError ||
    isRetryableConnectionError(error)
  )
    return new WorkspaceSourceError("unavailable")
  return error
}

/**
 * A conversation code as the workspace's reason, answered for every code: a
 * `switch` with a declared return does not compile until the next code the
 * gateway adds is answered (gate 11). A code that says where the target
 * stands — no such conversation, no such review waiting — holds whatever this
 * command did. A refusal for good is `not-supported` only when the client says
 * the command certainly did nothing (`uncertain` false); one that may have
 * taken effect is `unavailable`, which `failure.ts` keeps for that.
 */
function reasonFor(
  code: ConversationErrorCode,
  uncertain: boolean,
): WorkspaceFailureReason {
  switch (code) {
    // The gateway holds no such conversation for this caller, or it was deleted.
    case ConversationErrorCode.ConversationNotFound:
    case ConversationErrorCode.ConversationDeleted:
      return "unknown-session"
    // The review was answered, withdrawn, or never asked.
    case ConversationErrorCode.StalePermission:
      return "not-waiting"
    // Refused for good, whatever the timing: the gateway cannot do this here,
    // this window asked it wrongly, or it cannot read this conversation's
    // saved state or its configuration changed under it — asking again
    // changes nothing.
    case ConversationErrorCode.AgentNotConfigured:
    case ConversationErrorCode.AgentUnsupported:
    case ConversationErrorCode.ConversationsNotConfigured:
    case ConversationErrorCode.ModelUnavailable:
    case ConversationErrorCode.ImageInputUnsupported:
    case ConversationErrorCode.ApprovalModeUnavailable:
    case ConversationErrorCode.UnknownMethod:
    case ConversationErrorCode.InvalidRequest:
    case ConversationErrorCode.SubmissionConflict:
    case ConversationErrorCode.ConversationStateUnreadable:
    case ConversationErrorCode.ConversationConfigurationChanged:
      return uncertain ? "unavailable" : "not-supported"
    // Not done now, or not known to be: busy, starting, stopped, storage or
    // audit that did not answer, an outcome the gateway could not settle.
    case ConversationErrorCode.ApprovalModeNotApplied:
    case ConversationErrorCode.ApprovalModeUncertain:
    case ConversationErrorCode.ApprovalRequestConflict:
    case ConversationErrorCode.TurnRunning:
    case ConversationErrorCode.ConversationCapacity:
    case ConversationErrorCode.ConversationClosed:
    case ConversationErrorCode.ConversationStorageUnavailable:
    case ConversationErrorCode.TemporarilyUnavailable:
    case ConversationErrorCode.AuditUnavailable:
    case ConversationErrorCode.SubmissionUnresolved:
    case ConversationErrorCode.AgentStartupDeadline:
    case ConversationErrorCode.AgentOperationFailed:
    case ConversationErrorCode.AttachmentNotFound:
    case ConversationErrorCode.AttachmentUnavailable:
    case ConversationErrorCode.AttachmentCapacity:
    case ConversationErrorCode.AttachmentStorageUnavailable:
    case ConversationErrorCode.AttachmentCleanupUnavailable:
    case ConversationErrorCode.ConversationErasureIncomplete:
      return "unavailable"
    // An MCP App's own calls: this source makes none, so none of these can
    // answer it; one that did would be no answer it knows.
    case ConversationErrorCode.McpAppUnknown:
    case ConversationErrorCode.McpServerMismatch:
    case ConversationErrorCode.McpToolNotForApp:
    case ConversationErrorCode.McpSessionUnavailable:
    case ConversationErrorCode.McpApprovalDenied:
    case ConversationErrorCode.McpApprovalExpired:
    case ConversationErrorCode.McpCancelled:
    case ConversationErrorCode.McpRequestTooLarge:
    case ConversationErrorCode.McpResultTooLarge:
    case ConversationErrorCode.McpTimedOut:
    case ConversationErrorCode.McpRemoteError:
    case ConversationErrorCode.McpUnauthorized:
    case ConversationErrorCode.McpUnreachable:
    case ConversationErrorCode.McpInsufficientScope:
      return "unavailable"
  }
}
