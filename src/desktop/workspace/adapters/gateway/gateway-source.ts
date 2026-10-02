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
 *   has read (`transcript`) that runs, waits on the person, or changed since.
 * - **Revisions are minted here.** The gateway's view revision is opaque and
 *   its list rows carry none, so this adapter counts: one counter per
 *   session's summary, one per conversation, each from 1, moved on when what
 *   it maps changes. A removal is the summary's next count. Opaque revisions
 *   cannot order two answers, so lists run one at a time, and so do reads of
 *   one conversation: an answer applies in the order it was asked, and one
 *   that came after its call timed out is let go.
 * - **Every call settles on its own timer** (`timing.callMs`, from `clock`),
 *   connection included, rejecting `unavailable` when it runs out.
 * - **Refusals are typed.** The gateway's codes become `WorkspaceSourceError`
 *   reasons (`refusalOf`); a fault that is no answer at all is passed on as
 *   it is, for `failureReason` to log.
 * - **Resync.** `{ kind: "resync" }` goes out when a connection comes back
 *   (the client reconnected, or a new one was made after the last closed),
 *   and on the first list that answers after a poll failed.
 * - **What the gateway does not keep**: pins and "always" answers are
 *   refused as `not-supported`; there is no unread mark, so `markRead` has
 *   nothing to do. Nor does it take an initiator: it records each answer
 *   and archive as this window's authenticated caller, and cannot tell the
 *   person from an agent dispatching the same command — a gap in the record
 *   the in-memory source keeps, said here until the protocol carries it.
 *
 * It owns the client it connects (`connect`), and `dispose` closes it.
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
import { agentForProvider } from "../../../model/composer-options"
import {
  WorkspaceSourceError,
  type ApprovalScope,
  type OutgoingMessage,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { Transcript } from "../../model/transcript"
import type {
  ModelRef,
  SessionSummary,
  WorkspaceIndex,
} from "../../model/workspace-index"
import {
  gatewayChannel,
  gatewaySection,
  modelFor,
  reviewOf,
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

export interface GatewayTiming {
  /** How long any call may take before it settles as `unavailable`. */
  readonly callMs: number
  /** How long the poller rests between rounds. */
  readonly pollMs: number
}

/**
 * Longer than the client's own 30-second request timeout, so its typed
 * answer arrives first when there is one; the adapter's timer is for a call
 * that gets none.
 */
export const defaultGatewayTiming: GatewayTiming = { callMs: 35_000, pollMs: 1_000 }

export interface GatewaySource extends WorkspaceSource {
  /** Stops polling, refuses every later call, and closes the client it connected. */
  dispose(): void
}

/** What one conversation's latest read left: its view, its count, and the row it was read against. */
interface Read {
  readonly view: ConversationView
  readonly transcript: Transcript
  readonly against: ConversationSummary | undefined
}

/** A summary as held: what it said, at what count, and whether it was taken out. */
interface Held {
  readonly summary: SessionSummary
  readonly removed: boolean
}

export function gatewaySource(options: {
  /** Connects a client: called again after a failed attempt, or once the last one closed. */
  readonly connect: () => Promise<GatewayClient>
  readonly clock: GatewayClock
  readonly timing?: GatewayTiming
}): GatewaySource {
  const { clock } = options
  const timing = options.timing ?? defaultGatewayTiming
  const listeners = new Set<(update: WorkspaceUpdate) => void>()
  let disposed = false

  // Each session's summary counter, kept after a removal so a later listing outranks it.
  const summaryCounts = new Map<string, number>()
  const summaries = new Map<string, Held>()
  const rows = new Map<string, ConversationSummary>()
  // Each conversation's counter, the opaque revision it was last moved on for, and the read.
  const transcriptCounts = new Map<string, number>()
  const reads = new Map<string, Read>()
  // When each message was first seen, per session: the gateway's view has no times.
  const firstSeen = new Map<string, Map<string, number>>()
  // The model the window sent each session's message with.
  const sentModels = new Map<string, ModelRef>()
  // Conversations the window has read, and so wants kept current.
  const watched = new Set<string>()
  // Conversations opened on the current connection (`create`), shared by callers.
  const opened = new Map<string, Promise<void>>()

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

  // Polls failed since the last resync: the next list that answers says resync.
  let gap = false
  const resync = () => {
    gap = false
    opened.clear()
    emit({ kind: "resync" })
  }

  /** Settles `work` within the call budget, rejecting `unavailable` when it runs out. */
  const within = <T>(work: () => Promise<T>): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      if (disposed) return reject(new WorkspaceSourceError("unavailable"))
      const cancel = clock.after(timing.callMs, () =>
        reject(new WorkspaceSourceError("unavailable")),
      )
      Promise.resolve()
        .then(work)
        .then(
          (value) => {
            cancel()
            if (disposed) reject(new WorkspaceSourceError("unavailable"))
            else resolve(value)
          },
          (error: unknown) => {
            cancel()
            reject(refusalOf(error))
          },
        )
    })

  // The connection: one client at a time, connected on first need.
  let current: { client: GatewayClient; off: () => void } | undefined
  let connecting: Promise<GatewayClient> | undefined
  let connectedBefore = false
  /** Takes a client that connected as the current one. */
  const adopt = (connected: GatewayClient) => {
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
    // A connection after another is a reconnect: what it missed is read again.
    if (connectedBefore) resync()
    connectedBefore = true
  }
  /**
   * The current client, or one connecting for every caller at once. An
   * attempt has the call budget too: one that outlasts it is given up — the
   * next call tries again — and a client it brings late is closed unused.
   */
  const client = (): Promise<GatewayClient> => {
    if (disposed) return Promise.reject(new WorkspaceSourceError("unavailable"))
    if (current) return Promise.resolve(current.client)
    if (connecting) return connecting
    const attempt = new Promise<GatewayClient>((resolve, reject) => {
      let over = false
      const giveUp = () => {
        over = true
        reject(new WorkspaceSourceError("unavailable"))
      }
      const cancel = clock.after(timing.callMs, giveUp)
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
          // Not reaching the gateway is an answer the window can show; why is logged.
          console.warn("Could not connect to the gateway", error)
          giveUp()
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

  // Lists run one at a time, and so do reads of one conversation.
  let listing: Promise<unknown> = Promise.resolve()
  const reading = new Map<string, Promise<unknown>>()
  const inTurn = <T>(after: Promise<unknown>, work: () => Promise<T>) => {
    const turn = after.then(work, work)
    return { turn, settled: turn.then(noop, noop) }
  }

  const modelOf = (sessionId: string) =>
    modelFor(sentModels.get(sessionId), reads.get(sessionId)?.view)

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
    if (held && !held.removed && sameSummary(held.summary, said)) return
    const revision = (summaryCounts.get(sessionId) ?? 0) + 1
    summaryCounts.set(sessionId, revision)
    const summary: SessionSummary = { ...said, revision }
    summaries.set(sessionId, { summary, removed: false })
    emit({ kind: "session", session: summary })
  }

  /** Takes a session out, at its summary's next count; one already out stays as it is. */
  const remove = (sessionId: string): void => {
    const held = summaries.get(sessionId)
    if (held?.removed) return
    const revision = (summaryCounts.get(sessionId) ?? 0) + 1
    summaryCounts.set(sessionId, revision)
    if (held) summaries.set(sessionId, { summary: held.summary, removed: true })
    rows.delete(sessionId)
    watched.delete(sessionId)
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
      for (const [sessionId, held] of summaries)
        if (!held.removed && !listed.has(sessionId)) remove(sessionId)
  }

  /** Lists in turn, applying the answer; the list's own failures are the caller's. */
  const list = (): Promise<void> => {
    const { turn, settled } = inTurn(listing, () =>
      within(async () => (await client()).conversation.list()).then(applyList),
    )
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

  /** Applies a read: a changed view is the conversation's next count, and is said. */
  const applyRead = (sessionId: string, view: ConversationView): Transcript => {
    const held = reads.get(sessionId)
    if (held && held.view.revision === view.revision) {
      reads.set(sessionId, { ...held, against: rows.get(sessionId) })
      return held.transcript
    }
    const revision = (transcriptCounts.get(sessionId) ?? 0) + 1
    transcriptCounts.set(sessionId, revision)
    const transcript = transcriptFrom(view, revision, seenIn(sessionId))
    reads.set(sessionId, { view, transcript, against: rows.get(sessionId) })
    emit({ kind: "transcript", transcript })
    // The summary follows what the read says: an approval waiting, the model it runs on.
    publish(sessionId)
    return transcript
  }

  /** Reads one conversation in its turn; an answer after its call timed out is let go. */
  const read = (sessionId: string): Promise<Transcript> => {
    const { turn, settled } = inTurn(reading.get(sessionId) ?? Promise.resolve(), () =>
      within(async () => (await client()).conversation.read(sessionId)).then((view) =>
        applyRead(sessionId, view),
      ),
    )
    reading.set(sessionId, settled)
    return turn
  }

  /** Opens a conversation on this connection, once; a failed open is tried again next time. */
  const open = (sessionId: string, start?: OutgoingMessage): Promise<void> => {
    const known = opened.get(sessionId)
    if (known) return known
    const model = start?.model
    const agent = model && agentForProvider(model.provider)
    const opening = client()
      .then((connected) =>
        connected.conversation.create({
          conversationId: sessionId,
          ...(agent ? { agent } : {}),
          ...(model ? { model: model.modelId } : {}),
        }),
      )
      .then(noop)
    opened.set(sessionId, opening)
    opening.catch(() => {
      if (opened.get(sessionId) === opening) opened.delete(sessionId)
    })
    return opening
  }

  /** Whether a read conversation should be read again this round. */
  const stale = (sessionId: string): boolean => {
    const last = reads.get(sessionId)
    const row = rows.get(sessionId)
    // Not listed yet — just begun — or never read: read it.
    if (!last || !row) return true
    if (row.running || last.transcript.approval || last.transcript.activity) return true
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
      await list()
      if (gap) resync()
      for (const sessionId of [...watched]) {
        if (disposed || listeners.size === 0) break
        if (!stale(sessionId)) continue
        try {
          await read(sessionId)
        } catch (error) {
          // A conversation the gateway no longer holds is not watched; any
          // other failure is a gap the next list resyncs.
          if (error instanceof WorkspaceSourceError && error.reason === "unknown-session")
            watched.delete(sessionId)
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

  const waitingReview = async (sessionId: string, approvalId: string) => {
    const review = reviewOf(approvalId)
    if (!review) throw new WorkspaceSourceError("not-waiting")
    await read(sessionId)
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
    within(async () => {
      const permission = await waitingReview(sessionId, approvalId)
      const option = permission.options.find((offered) => offered.effect === effect)
      if (!option) throw new WorkspaceSourceError("not-supported")
      await (
        await client()
      ).conversation.answer(
        sessionId,
        permission.executionId,
        permission.permissionId,
        option.id,
      )
      // Resolves once the conversation that no longer asks is said; if that
      // read fails, the answer still stands, and the next list resyncs.
      await read(sessionId).catch(() => {
        gap = true
      })
    })

  return {
    index: () =>
      within(async () => {
        await list()
        const index: WorkspaceIndex = {
          sections: [gatewaySection],
          channels: [gatewayChannel],
          sessions: [...summaries.values()]
            .filter((held) => !held.removed)
            .map((held) => held.summary),
        }
        return index
      }),
    transcript: (sessionId) =>
      within(async () => {
        watched.add(sessionId)
        await open(sessionId)
        return read(sessionId)
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
      within(async () => {
        sentModels.set(message.sessionId, message.model)
        await open(message.sessionId, message.start ? message : undefined)
        await (
          await client()
        ).conversation.send(message.sessionId, message.text, [], [], {
          // The message's id names the same turn however often it is sent.
          executionId: message.messageId,
          requestId: message.messageId,
        })
        watched.add(message.sessionId)
        publish(message.sessionId)
      }),
    approve: (sessionId: string, approvalId: string, scope: ApprovalScope) =>
      scope === "always"
        ? // The gateway never offers a persistent answer (ADR 344's offer policy).
          Promise.reject(new WorkspaceSourceError("not-supported"))
        : answer(sessionId, approvalId, "allow"),
    deny: (sessionId, approvalId) => answer(sessionId, approvalId, "deny"),
    // The gateway keeps no pin.
    setPinned: () => Promise.reject(new WorkspaceSourceError("not-supported")),
    archive: (sessionId) =>
      within(async () => {
        // In the list's turn, so no list asked before it can list the session again.
        const { turn, settled } = inTurn(listing, async () => {
          await (await client()).conversation.archive(sessionId)
          remove(sessionId)
        })
        listing = settled
        await turn
      }),
    // The gateway keeps no unread mark: every summary is read already.
    markRead: () => within(() => Promise.resolve()),
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

/**
 * The source's reason for a failed call, by the error's type: the gateway's
 * conversation codes it can name, `unavailable` for no answer, and anything
 * else — a fault, such as an answer the client could not read — as it is.
 */
export function refusalOf(error: unknown): unknown {
  if (error instanceof WorkspaceSourceError) return error
  const code =
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError
      ? error.code
      : error instanceof NessaRpcError
        ? conversationErrorCode(error.code)
        : undefined
  if (code !== undefined) return new WorkspaceSourceError(reasonFor(code))
  if (
    error instanceof NessaConversationMutationError ||
    error instanceof NessaConversationControlError ||
    error instanceof NessaRpcError ||
    error instanceof NessaConnectionClosedError ||
    error instanceof NessaSessionUnavailableError ||
    isRetryableConnectionError(error)
  )
    return new WorkspaceSourceError("unavailable")
  return error
}

/** A conversation code as the workspace's reason. */
function reasonFor(code: ConversationErrorCode): WorkspaceFailureReason {
  switch (code) {
    case ConversationErrorCode.ConversationNotFound:
    case ConversationErrorCode.ConversationDeleted:
      return "unknown-session"
    case ConversationErrorCode.StalePermission:
      return "not-waiting"
    default:
      return "unavailable"
  }
}
