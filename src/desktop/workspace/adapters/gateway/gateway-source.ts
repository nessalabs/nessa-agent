/**
 * The `WorkspaceSource` over the gateway (#248): the window's sessions are
 * the caller's gateway conversations, read through `@nessa/client`.
 *
 * The design and its orderings are a table on #248; each row has a test in
 * `gateway-source.test.ts`. In short:
 *
 * - **No push stream.** The gateway sends no conversation events, so the
 *   stream is a poller, running while anyone listens: `conversation.list`
 *   for summaries, then `conversation.read` for each conversation the window
 *   has read (`transcript`) that runs, waits on the person, has an app's
 *   call unanswered (`appCall`, #436), or changed since.
 * - **Revisions are minted here.** The gateway's view revision is opaque and
 *   its list rows carry none, so this adapter counts: one counter per
 *   session's summary, one per conversation, each from 1, moved on when what
 *   it maps changes. A removal is the summary's next count. Opaque revisions
 *   cannot order two answers, so lists run one at a time, and so do reads of
 *   one conversation: an answer applies in the order it was asked, and one
 *   that came after its call timed out is let go.
 * - **Every call settles on its own timer** (`timing.callMs`, from `clock`),
 *   connection included, rejecting `unavailable` when it runs out; and a
 *   call that has settled sends nothing consequential afterwards (`dispatch`).
 * - **Only a first message sends `create`.** The gateway resolves the
 *   conversation itself for each read and message — starting its agent if it
 *   is not running — so this source keeps nothing per connection.
 * - **Refusals are typed.** The gateway's codes become `WorkspaceSourceError`
 *   reasons (`refusalOf`); a fault that is no answer at all is passed on as
 *   it is, for `failureReason` to log. A connection that could not be made
 *   is `signed-out` or `unavailable` by why (`connectFailure`, #419). A read
 *   or the index that is refused is also traced (`noteReadAsked`,
 *   `noteReadRefused`): the session, whether it was the index or a
 *   conversation, the reason, and the gateway or socket hint already on the
 *   error. The ask is a debug line; the refusal is a warning the dev console
 *   already forwards.
 * - **Resync.** `{ kind: "resync" }` goes out when a connection comes back
 *   (the client reconnected, or a new one was made after the last closed),
 *   and on the first list that answers after a poll or an index read failed.
 * - **A failed connect is waited out.** For `timing.reconnectRounds` poll
 *   rounds after it, neither the poller nor an MCP App connects again; a
 *   person's call connects at once (S10–S16 on #419).
 * - **What the gateway does not keep**: pins and "always" answers are
 *   refused as `not-supported`, and so is a message asking a conversation
 *   for another model than the one the gateway says it runs (before a read
 *   says, it cannot be told, and the message goes); there is no unread
 *   mark, so `markRead` has nothing to do. Nor does it take an initiator: it
 *   records each answer and archive as this window's authenticated caller,
 *   and cannot tell the person from an agent dispatching the same command —
 *   a gap in the record the in-memory source keeps, said here until the
 *   protocol carries it.
 * - **What it cannot see without reading**: a list row says whether a
 *   conversation runs, not whether it waits on the person, so a summary says
 *   `needs-you` only for a conversation the window has read; and a question
 *   the agent asks (`view.questions`) has no place in the workspace's model
 *   and is not shown; nor is a review the gateway withholds from its view
 *   (`interactionViewError`) — the session shows no approval while it waits.
 * - **For MCP Apps (#384)**: a widget part is named in its conversation
 *   (`gatewayToolWidget`, from `transcriptFrom`); each view applied is handed
 *   to `apps.observe`, in the order read — opaque revisions cannot order them
 *   there — and a read the gateway refuses as `conversation_deleted` to
 *   `apps.forget`. Not a removal: a conversation missing from a complete list
 *   is archived or deleted, which a list cannot tell apart, and a closed one
 *   reopens under its id (the plan is on #248). An app's call goes through
 *   `appCall`, so the review it may wait on is read even after the turn
 *   ended: a review an app opens changes nothing in the list row.
 *
 * It owns the client it connects (`connect`), and `dispose` closes it; an
 * app's calls go on that client too (`connected`).
 */
import {
  ConversationErrorCode,
  conversationErrorCode,
  isRetryableConnectionError,
  NessaConnectionClosedError,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  NessaSessionUnavailableError,
  type ConnectionState,
  type ConversationApi,
  type ConversationListResult,
  type ConversationSummary,
  type ConversationView,
} from "@nessa/client"
import { isSignedOut } from "../../../../session"
import { agentForProvider } from "../../../model/composer-options"
import {
  WorkspaceSourceError,
  type ApprovalScope,
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

/** What the adapter asks of a gateway client: its conversations, and how its connection stands. */
export interface GatewayClient {
  readonly conversation: Pick<
    ConversationApi,
    "list" | "read" | "create" | "send" | "answer" | "archive"
  >
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
 * when none is held (`client`): a person, the poller's list, an MCP App, or
 * a read that uses only what is held — the poller's reads, and the read that
 * follows an answer.
 */
type Caller = "person" | "poller" | "app" | "held"

/**
 * What each caller may do when no client is held and none is connecting:
 * connect `always`, `unless-waiting` out a failed connect, or `never`; and
 * whether its refusal while waiting counts the wait down a round. Total, so a
 * caller added later does not compile until its rule is chosen (gate 11).
 */
const callerRules: Record<
  Caller,
  {
    readonly connects: "always" | "unless-waiting" | "never"
    readonly countsDown: boolean
  }
> = {
  person: { connects: "always", countsDown: false },
  poller: { connects: "unless-waiting", countsDown: true },
  app: { connects: "unless-waiting", countsDown: false },
  held: { connects: "never", countsDown: false },
}

export interface GatewayTiming {
  /** How long any call may take before it settles as `unavailable`. */
  readonly callMs: number
  /** How long the poller rests between rounds. */
  readonly pollMs: number
  /**
   * How many poll rounds pass after a failed connect before the poller, or
   * an MCP App, connects again (S10, #419). Rounds, not a time: the clock
   * cannot move them.
   */
  readonly reconnectRounds: number
}

/**
 * The adapter's own budget for each call, whatever the client is doing: a
 * call of several requests (a send is `create` then `send`) shares it, and
 * one still waiting on the client when it runs out settles `unavailable`.
 */
export const defaultGatewayTiming: GatewayTiming = {
  callMs: 35_000,
  pollMs: 1_000,
  reconnectRounds: 5,
}

export interface GatewaySource<
  C extends GatewayClient = GatewayClient,
> extends WorkspaceSource {
  /**
   * The client this source holds, or one connecting, within the call budget:
   * rejects `unavailable` once disposed, when none connects in time, or
   * while the source waits out a failed connect (S14); and `signed-out` when
   * the connect it made or joined is refused that way.
   */
  connected(): Promise<C>
  /**
   * Makes one of an app's calls in conversation `conversationId`, and,
   * while the window follows that conversation, reads it each round until
   * the call is answered (a conversation taken out is not read, P9). The call may
   * wait on a review the gateway opens for it, which changes nothing a list
   * row says: without this, a conversation whose turn has ended is not read
   * again, and its review is not drawn (#436). The call settles as `call`
   * does.
   */
  appCall<T>(conversationId: string, call: () => Promise<T>): Promise<T>
  /** Stops polling, refuses every later call, and closes the client it connected. */
  dispose(): void
}

/** Who is told each conversation view as it is read, and each conversation deleted (#384). */
export interface GatewayViewObserver {
  /** A view, in the order its conversation's reads were applied. */
  observe(view: ConversationView): void
  /** The gateway said the conversation was deleted; its id is never used again. */
  forget(conversationId: string): void
}

/** What one conversation's latest read left: its view, its count, and the row it was read against. */
interface Read {
  readonly view: ConversationView
  readonly transcript: Transcript
  readonly against: ConversationSummary | undefined
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
  // Each conversation's counter, the opaque revision it was last moved on for, and the read.
  const transcriptCounts = new Map<string, number>()
  const reads = new Map<string, Read>()
  // When each message was first seen, per session: the gateway's view has no times.
  const firstSeen = new Map<string, Map<string, number>>()
  // How often each session was taken out: a read asked before its latest removal is let go (S3c).
  const removals = new Map<string, number>()
  // Conversations the window has read, and so wants kept current. Watching
  // ends in two places only: `remove`, and a read the gateway answers with no
  // such conversation (S8) or a refusal for good (R11) — until Try Again.
  const watched = new Set<string>()
  // Each conversation's app calls not yet answered (`appCall`): while there
  // is one, the conversation is read each round (#436, P2–P7).
  const appCalls = new Map<string, number>()

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

  // The apps' fault is theirs: it neither stops the read nor the source.
  const tellApps = (tell: () => void) => {
    try {
      tell()
    } catch (error) {
      console.error("The window's MCP Apps failed to take a conversation view", error)
    }
  }

  // Polls or index reads failed since the last resync: the next list that
  // answers says resync. An index read the window did not get is a gap too:
  // a poll that answers after it, the gateway having come up between them,
  // would otherwise leave the window on its failure (S9, #419).
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
  // After a connect that failed — refused, or out of time (S17) — nothing
  // in the background connects again for `reconnectRounds` poll rounds (S10,
  // S14): each connect asks the host for the gateway and its credential,
  // and the gateway to authenticate, so a window that cannot connect must
  // not ask every round. A person's call connects at once, and if it fails
  // the wait starts again (S12); a connect that succeeds ends it. The next
  // connect after the gateway comes back is so at most `reconnectRounds + 1`
  // rounds away (S16). Who may connect, and the wait's counting, are
  // `client`'s alone to decide.
  let roundsToWait = 0
  const connectFailed = () => {
    roundsToWait = timing.reconnectRounds
  }
  /** Takes a client that connected as the current one. */
  const adopt = (connected: C) => {
    const off = connected.onConnectionStateChange((state) => {
      if (current?.client !== connected) return
      if (state.status === "connected") resync()
      else if (state.status === "closed") {
        // Gone for good: the next call connects again.
        current.off()
        current = undefined
        gap = true
      }
    })
    current = { client: connected, off }
    roundsToWait = 0
    // A connection after another is a reconnect: what it missed is read again.
    if (connectedBefore) resync()
    connectedBefore = true
  }
  /**
   * The current client, or one connecting for every caller at once — and,
   * when there is neither, whether to connect, by who is asking: the one
   * place that is decided (#419).
   *
   * - `person`: a foreground call — the index (Try Again, or the store's
   *   own re-read on a resync), opening a session, a message, an answer and
   *   the read of its review, an archive — connects at once.
   * - `poller`: the poller's list connects unless the source is waiting out
   *   a failed connect; each list refused while it waits counts the wait
   *   down a round (S10).
   * - `app`: an MCP App connects unless the source is waiting, and its
   *   refusals count nothing (S14).
   * - `held`: the poller's reads, and the read that follows an answer, use
   *   the client held and never connect in its place (S15, W3′).
   *
   * Any of them joins a connect already on its way (S6, S18). An attempt has
   * the call budget too: one that outlasts it is given up — and counts as a
   * failed connect (S17) — and a client it brings late is closed unused.
   */
  const client = (who: Caller): Promise<C> => {
    if (disposed) return Promise.reject(new WorkspaceSourceError("unavailable"))
    if (current) return Promise.resolve(current.client)
    if (connecting) return connecting
    const rule = callerRules[who]
    if (rule.connects === "never")
      return Promise.reject(new WorkspaceSourceError("unavailable"))
    if (rule.connects === "unless-waiting" && roundsToWait > 0) {
      if (rule.countsDown) roundsToWait -= 1
      return Promise.reject(new WorkspaceSourceError("unavailable"))
    }
    const attempt = new Promise<C>((resolve, reject) => {
      let over = false
      const giveUp = (reason: WorkspaceFailureReason = "unavailable") => {
        over = true
        connectFailed()
        reject(new WorkspaceSourceError(reason))
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

  // Lists run one at a time, and so do reads of one conversation.
  let listing: Promise<unknown> = Promise.resolve()
  const reading = new Map<string, Promise<unknown>>()
  const inTurn = <T>(after: Promise<unknown>, work: () => Promise<T>) => {
    const turn = after.then(work, work)
    return { turn, settled: turn.then(noop, noop) }
  }

  /** The model a session is known to run on: the gateway's own word for it, from its last read. */
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

  /** Takes a session out, at its summary's next count; one already out stays as it is. */
  const remove = (sessionId: string): void => {
    if (takenOut(sessionId)) return
    const revision = (summaryCounts.get(sessionId) ?? 0) + 1
    summaryCounts.set(sessionId, revision)
    // Taken out whether or not a list had named it yet: an archive of a
    // session just begun here is remembered too.
    removedIds.add(sessionId)
    removals.set(sessionId, (removals.get(sessionId) ?? 0) + 1)
    rows.delete(sessionId)
    watched.delete(sessionId)
    // Its last read goes with it: listed again, nothing it said then speaks for it.
    reads.delete(sessionId)
    emit({ kind: "session-removed", sessionId, revision })
  }

  /**
   * Applies a list: each row said again, and — only when the list is complete
   * — a session it does not name taken out. An incomplete list proves nothing
   * of what it leaves out.
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
   * Lists in turn, applying the answer; the list's own failures are the
   * caller's. One whose caller was answered — while it waited its turn, or
   * while its list was on its way — asks nothing more, and applies nothing
   * (R10).
   */
  const list = (who: Caller, caller: () => boolean = always): Promise<void> => {
    const { turn, settled } = inTurn(listing, async () => {
      if (!caller()) throw new WorkspaceSourceError("unavailable")
      const result = await within(async () => (await client(who)).conversation.list(), {
        subject: "index",
      })
      if (!caller()) throw new WorkspaceSourceError("unavailable")
      applyList(result)
    })
    listing = settled
    return turn
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
   * Applies a read: a changed view is the conversation's next count, and is
   * said. It is filed against the row held when it was asked (`against`), not
   * one a list brought while it was on its way: a change that row says is
   * then still unread, and the next round reads it (R7).
   */
  const applyRead = (
    sessionId: string,
    view: ConversationView,
    against: ConversationSummary | undefined,
  ): Transcript => {
    const held = reads.get(sessionId)
    if (held && held.view.revision === view.revision) {
      reads.set(sessionId, { ...held, against })
      return held.transcript
    }
    const revision = (transcriptCounts.get(sessionId) ?? 0) + 1
    transcriptCounts.set(sessionId, revision)
    const transcript = transcriptFrom(view, revision, seenIn(sessionId))
    reads.set(sessionId, { view, transcript, against })
    tellApps(() => options.apps?.observe(view))
    emit({ kind: "transcript", transcript })
    // The summary follows what the read says: an approval waiting, the model it runs on.
    publish(sessionId)
    return transcript
  }

  /**
   * The port's rule for a session this source has taken out — archived here
   * or elsewhere, or missing from a complete list: it holds no such session,
   * reads none and begins none under its id (`failure.ts`, R8).
   */
  const held = (sessionId: string) => {
    if (takenOut(sessionId)) throw new WorkspaceSourceError("unknown-session")
  }

  /**
   * Reads one conversation in its turn; an answer after its call timed out
   * is let go. One that crossed a removal of its session speaks for a
   * listing no longer held, so it is not applied: a session held again is
   * asked again, once. One still taken out answers `unknown-session`; one
   * held but crossed again answers `unavailable` — not done now, try again
   * (S3c, S8) — let go is not gone, and not forever. One whose caller was
   * answered — while it waited its turn, or while its read was on its way —
   * asks nothing more, and applies nothing, not even a gone (R5, R10).
   */
  const read = (
    sessionId: string,
    who: Caller,
    caller: () => boolean = always,
  ): Promise<Transcript> => {
    const { turn, settled } = inTurn(
      reading.get(sessionId) ?? Promise.resolve(),
      async () => {
        for (let asked = 0; asked < 2; asked++) {
          try {
            held(sessionId)
          } catch (error) {
            noteTraced({ subject: "conversation", sessionId }, error, error)
            throw error
          }
          if (!caller()) {
            const error = new WorkspaceSourceError("unavailable")
            noteTraced({ subject: "conversation", sessionId }, error, error)
            throw error
          }
          const against = rows.get(sessionId)
          const removed = removals.get(sessionId) ?? 0
          let view: ConversationView
          // Whether the gateway answered that the conversation was deleted:
          // `refusalOf` keeps only that it is gone.
          let deleted = false
          try {
            view = await within(
              async () =>
                (await client(who)).conversation
                  .read(sessionId)
                  .catch((error: unknown) => {
                    deleted = deletedConversation(error)
                    throw error
                  }),
              { subject: "conversation", sessionId },
            )
          } catch (error) {
            if (!caller()) throw new WorkspaceSourceError("unavailable")
            if (!gone(error)) throw error
            // The gateway's own word that the conversation is gone takes the
            // session out, even where no complete list would (R9) — unless it
            // crossed a removal, when it speaks for a listing no longer held:
            // a session listed again is asked again (S3c).
            if ((removals.get(sessionId) ?? 0) !== removed) continue
            remove(sessionId)
            // Deleted, its apps' calls go with it; not found may be this
            // caller's alone, and says nothing of what the apps hold.
            if (deleted) tellApps(() => options.apps?.forget(sessionId))
            throw error
          }
          if (!caller()) throw new WorkspaceSourceError("unavailable")
          if ((removals.get(sessionId) ?? 0) === removed)
            return applyRead(sessionId, view, against)
        }
        try {
          held(sessionId)
        } catch (error) {
          noteTraced({ subject: "conversation", sessionId }, error, error)
          throw error
        }
        const error = new WorkspaceSourceError("unavailable")
        noteTraced({ subject: "conversation", sessionId }, error, error)
        throw error
      },
    )
    reading.set(sessionId, settled)
    return turn
  }

  /** Whether a read conversation should be read again this round. */
  const stale = (sessionId: string): boolean => {
    const last = reads.get(sessionId)
    const row = rows.get(sessionId)
    if (!last) return true
    const live = Boolean(
      last.transcript.approval || last.transcript.activity || appCalls.has(sessionId),
    )
    // Not listed — just begun, or past an incomplete list: read while it is live (S7).
    if (!row) return live
    if (row.running || live) return true
    return (
      last.against?.updatedAtMs !== row.updatedAtMs ||
      last.against.running !== row.running
    )
  }

  // The poller: one round at a time, while anyone listens.
  let cancelPoll: (() => void) | undefined
  let polling = false
  const round = async () => {
    polling = true
    try {
      // While the source waits out a failed connect, the list is refused
      // without asking anything, and the round ends (S10, `client`).
      await list("poller")
      if (gap) resync()
      for (const sessionId of [...watched]) {
        if (disposed || listeners.size === 0) break
        // One taken out since the round began is refused by `read` itself (R8, S9).
        if (!stale(sessionId)) continue
        try {
          // On the list's client only: a client closed meanwhile refuses the
          // rest of the round, which the next round's list reads again (S15).
          await read(sessionId, "held")
        } catch (error) {
          // A conversation the gateway no longer holds, or will not read for
          // this window, is not watched — Try Again watches it again; any
          // other failure is a gap the next list resyncs (R11).
          if (gone(error) || refusedForGood(error)) watched.delete(sessionId)
          else gap = true
        }
      }
    } catch {
      gap = true
    } finally {
      polling = false
      schedule()
    }
  }
  const schedule = () => {
    if (disposed || listeners.size === 0 || polling || cancelPoll) return
    cancelPoll = clock.after(timing.pollMs, () => {
      cancelPoll = undefined
      void round()
    })
  }
  const stopPolling = () => {
    cancelPoll?.()
    cancelPoll = undefined
  }

  const waitingReview = async (
    sessionId: string,
    approvalId: string,
    live: () => boolean,
  ) => {
    const review = reviewOf(approvalId)
    if (!review) throw new WorkspaceSourceError("not-waiting")
    await read(sessionId, "person", live)
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

  /** Answers a review with the option of `effect` it offers; none offered is not supported. */
  const answer = (sessionId: string, approvalId: string, effect: "allow" | "deny") =>
    within(async (live) => {
      const permission = await waitingReview(sessionId, approvalId, live)
      const option = permission.options.find((offered) => offered.effect === effect)
      if (!option) throw new WorkspaceSourceError("not-supported")
      await dispatch(live, (connected) =>
        connected.conversation.answer(
          sessionId,
          permission.executionId,
          permission.permissionId,
          option.id,
        ),
      )
      // Taken: the call resolves now, and the conversation after the answer
      // follows as an update (`ports.ts`). Read once more for it, not awaited,
      // so a slow read cannot report a taken answer failed; if it fails — or
      // the client has closed, which it does not connect again for (W3′) —
      // the answer still stands, and the next list resyncs (W3b, W3c) —
      // unless the session is gone, which is no gap (S5).
      read(sessionId, "held").catch((error: unknown) => {
        if (!gone(error)) gap = true
      })
    })

  return {
    index: () =>
      within(async (live) => {
        await list("person", live).catch((error: unknown) => {
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
    transcript: (sessionId) =>
      within(async (live) => {
        // A read sends no `create`. The session is followed from now on —
        // a read that fails is mended by the poller's next one — unless it is
        // gone: one taken out is refused and not followed (R3, R8).
        if (!takenOut(sessionId)) watched.add(sessionId)
        try {
          return await read(sessionId, "person", live)
        } catch (error) {
          if (gone(error) || refusedForGood(error)) watched.delete(sessionId)
          throw error
        }
      }),
    subscribe(listener) {
      if (disposed) return noop
      listeners.add(listener)
      schedule()
      return () => {
        listeners.delete(listener)
        if (listeners.size === 0) stopPolling()
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
          // send (W8). Not read yet, it cannot be told.
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
        if (!takenOut(message.sessionId)) watched.add(message.sessionId)
        publish(message.sessionId)
      }),
    approve: (sessionId: string, approvalId: string, scope: ApprovalScope) =>
      scope === "always"
        ? // No option the gateway shows reaches past its request: the projection
          // offers no review with one (nessa-server's projection test
          // `a_review_reaching_beyond_its_request_is_not_offered`).
          Promise.reject(new WorkspaceSourceError("not-supported"))
        : answer(sessionId, approvalId, "allow"),
    deny: (sessionId, approvalId) => answer(sessionId, approvalId, "deny"),
    // The gateway keeps no pin.
    setPinned: () => Promise.reject(new WorkspaceSourceError("not-supported")),
    archive: (sessionId) =>
      within(async (live) => {
        // In the list's turn, so no list asked before it can list the session again.
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
        await turn
      }),
    // The gateway keeps no unread mark: every summary is read already.
    markRead: () => within(() => Promise.resolve()),
    // An app calls on its own schedule, not a person's: in the background (S14).
    connected: () => within(() => client("app")),
    appCall(conversationId, call) {
      appCalls.set(conversationId, (appCalls.get(conversationId) ?? 0) + 1)
      // However it settles — answered, refused, or not sent at all — it is no longer asked.
      return Promise.resolve()
        .then(call)
        .finally(() => {
          const left = (appCalls.get(conversationId) ?? 1) - 1
          if (left > 0) appCalls.set(conversationId, left)
          else appCalls.delete(conversationId)
        })
    },
    dispose() {
      disposed = true
      stopPolling()
      listeners.clear()
      current?.off()
      current?.client.close()
      current = undefined
    },
  }
}

function noop() {}

/** The caller of a call no one waits on — the poller's own: always open. */
function always() {
  return true
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
 * Why no client connected, by the error's type: `signed-out` when the
 * gateway refused the credential (`isSignedOut`, the session's rule), so the
 * request was not sent; `unavailable` for everything else — no credential
 * to present, no answer, a gateway that is not running, or a host refusal,
 * which crosses IPC as a sentence that cannot be told apart by type.
 */
function connectFailure(error: unknown): WorkspaceFailureReason {
  return isSignedOut(error) ? "signed-out" : "unavailable"
}

/** Whether a failure is a refusal that asking again changes nothing of (`reasonFor`). */
function refusedForGood(error: unknown): boolean {
  return error instanceof WorkspaceSourceError && error.reason === "not-supported"
}

/** Which read the desktop asked for. `index` is `conversation.list`. */
type ReadTrace = { subject: "index" | "conversation"; sessionId?: string }

/**
 * The desktop asked to read. A debug line: a watched conversation is read
 * every poll, and the dev console forwards warnings, not this. No trace
 * means this call is not a read.
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

function readTrace(subject: "index" | "conversation", sessionId?: string) {
  return {
    subject,
    method: subject === "index" ? "conversation.list" : "conversation.read",
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
      return "unavailable"
  }
}
