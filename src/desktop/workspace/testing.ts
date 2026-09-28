/**
 * What the workspace's tests share: a small index, a source whose
 * every answer the test decides — success, a typed refusal, or silence until
 * released — and a real desktop store around it. Only tests import this.
 */
import { makeDesktopStore } from "../store"
import type {
  OutgoingMessage,
  RememberedFilter,
  WorkspaceSource,
  WorkspaceUpdate,
} from "./application/ports"
import { defaultFilter, type AgentsFilter } from "./model/overview/filter"
import type { WorkspaceFailureReason } from "./model/failure"
import type { WorkspaceRoom } from "../split-panes/model/pane-sizing"
import { WorkspaceSourceError } from "./application/ports"
import type {
  WorkspaceIndex,
  SessionStatus,
  SessionSummary,
} from "./model/workspace-index"
import type { Transcript } from "./model/transcript"
import { emptyTranscript } from "./model/transcript"

export const opus = { provider: "anthropic", modelId: "claude-opus-5" } as const
export const astra = { provider: "openai", modelId: "gpt-6-astra" } as const

export function summary(
  id: string,
  channelId: string,
  updatedAt: number,
  status: SessionStatus = "idle",
  extra: Partial<SessionSummary> = {},
): SessionSummary {
  return {
    id,
    channelId,
    title: `Session ${id}`,
    model: opus,
    status,
    startedAt: updatedAt - 10,
    updatedAt,
    preview: `Preview of ${id}`,
    pinned: false,
    unread: false,
    revision: 1,
    ...extra,
  }
}

/**
 * Two sections; `desktop` holds three sessions (one waiting, one running),
 * `gateway` one, `empty` none.
 */
export function testIndex(): WorkspaceIndex {
  return {
    sections: [
      { id: "starred", name: "Starred" },
      { id: "labs", name: "Labs" },
    ],
    channels: [
      { id: "desktop", name: "desktop", sectionId: "starred", private: false, topic: "" },
      { id: "empty", name: "empty", sectionId: "starred", private: false, topic: "" },
      { id: "gateway", name: "gateway", sectionId: "labs", private: true, topic: "" },
    ],
    sessions: [
      summary("a", "desktop", 300, "running"),
      summary("b", "desktop", 200, "needs-you", { unread: true }),
      summary("c", "desktop", 100),
      summary("d", "gateway", 400, "idle", { model: astra, pinned: true }),
    ],
  }
}

type Method = Exclude<keyof WorkspaceSource, "subscribe">

export interface FakeSource extends WorkspaceSource {
  /** Every call, in order, as `[method, ...arguments]`. */
  readonly calls: unknown[][]
  /** Makes a method refuse with this reason until cleared with `undefined`. */
  refuse(method: Method, reason: WorkspaceFailureReason | undefined): void
  /** Holds a method's answers until `release` is called. */
  hold(method: Method): void
  release(method: Method): Promise<void>
  /** Tells subscribers of an update, and keeps any summary it carries as current. */
  emit(update: WorkspaceUpdate): void
  transcripts: Map<string, Transcript>
}

export function fakeSource(index: WorkspaceIndex = testIndex()): FakeSource {
  const listeners = new Set<(update: WorkspaceUpdate) => void>()
  const refusals = new Map<Method, WorkspaceFailureReason>()
  const held = new Map<Method, (() => void)[] | null>()
  const calls: unknown[][] = []
  // The summaries as the source last said them, so pin and archive report the next revision.
  const archived = new Set<string>()
  const summaries = new Map(index.sessions.map((session) => [session.id, session]))
  const emit = (update: WorkspaceUpdate) => {
    if (update.kind === "session") {
      // Listed again by the source itself: it holds the session once more.
      summaries.set(update.session.id, update.session)
      archived.delete(update.session.id)
    }
    if (update.kind === "session-removed") {
      summaries.delete(update.sessionId)
      archived.add(update.sessionId)
    }
    listeners.forEach((listener) => listener(update))
  }
  const transcripts = new Map<string, Transcript>(
    index.sessions.map((session) => [
      session.id,
      // The source counts from 1; revision 0 is the window's own.
      { ...emptyTranscript(session.id), revision: 1 },
    ]),
  )

  // An approval answered: the conversation no longer asks it, one revision on.
  function answered(sessionId: string, approvalId: string) {
    if (archived.has(sessionId) || !summaries.has(sessionId))
      throw new WorkspaceSourceError("unknown-session")
    const transcript = transcripts.get(sessionId)
    if (transcript?.approval?.id !== approvalId)
      throw new WorkspaceSourceError("not-waiting")
    const next = { ...transcript, approval: null, revision: transcript.revision + 1 }
    transcripts.set(sessionId, next)
    emit({ kind: "transcript", transcript: next })
  }

  function answer<T>(method: Method, args: unknown[], value: () => T): Promise<T> {
    calls.push([method, ...args])
    const settle = () => {
      const reason = refusals.get(method)
      if (reason) throw new WorkspaceSourceError(reason)
      return value()
    }
    const waiting = held.get(method)
    if (!waiting) return Promise.resolve().then(settle)
    return new Promise<void>((resolve) => waiting.push(resolve)).then(settle)
  }

  return {
    calls,
    transcripts,
    refuse: (method, reason) => {
      if (reason) refusals.set(method, reason)
      else refusals.delete(method)
    },
    hold: (method) => held.set(method, []),
    release: async (method) => {
      const waiting = held.get(method) ?? []
      held.delete(method)
      for (const resolve of waiting) resolve()
      await Promise.resolve()
    },
    emit,
    index: () => answer("index", [], () => index),
    transcript: (sessionId) =>
      answer("transcript", [sessionId], () => {
        if (archived.has(sessionId)) throw new WorkspaceSourceError("unknown-session")
        const transcript = transcripts.get(sessionId)
        if (!transcript) throw new WorkspaceSourceError("unknown-session")
        return transcript
      }),
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    send: (message: OutgoingMessage) =>
      answer("send", [message], () => {
        // As the port says: a session it does not hold is begun only by a start,
        // and never under an archived id.
        if (
          archived.has(message.sessionId) ||
          (!summaries.has(message.sessionId) && !message.start)
        )
          throw new WorkspaceSourceError("unknown-session")
      }),
    // Like any source: the conversation that no longer asks reaches subscribers first.
    approve: (sessionId, approvalId, scope, initiator) =>
      answer("approve", [sessionId, approvalId, scope, initiator], () =>
        answered(sessionId, approvalId),
      ),
    deny: (sessionId, approvalId, initiator) =>
      answer("deny", [sessionId, approvalId, initiator], () =>
        answered(sessionId, approvalId),
      ),
    // Like any source: the change reaches subscribers before the call resolves.
    setPinned: (sessionId, pinned, initiator) =>
      answer("setPinned", [sessionId, pinned, initiator], () => {
        const session = summaries.get(sessionId)
        if (!session) throw new WorkspaceSourceError("unknown-session")
        emit({
          kind: "session",
          session: { ...session, pinned, revision: session.revision + 1 },
        })
      }),
    archive: (sessionId, initiator) =>
      answer("archive", [sessionId, initiator], () => {
        const session = summaries.get(sessionId)
        if (!session) throw new WorkspaceSourceError("unknown-session")
        emit({ kind: "session-removed", sessionId, revision: session.revision + 1 })
      }),
    markRead: (sessionId) => answer("markRead", [sessionId], () => undefined),
  }
}

/** The overview's filter kept in memory, as storage would keep it between launches. */
export function keptFilter(
  initial: AgentsFilter = defaultFilter,
): RememberedFilter & { readonly writes: AgentsFilter[] } {
  const writes: AgentsFilter[] = []
  return {
    writes,
    read: () => writes.at(-1) ?? initial,
    write: (filter) => void writes.push(filter),
  }
}

/** A grid with room for three columns of two panes, and a sidebar that could fold. */
export const roomyGrid: WorkspaceRoom = { width: 1100, height: 800, spare: 256 }

/**
 * A desktop store over `source`, with a clock at 1000, ids counting up, and
 * the panes' room as `measure` says — a roomy grid unless a test says
 * otherwise.
 */
export function testStore(
  source: WorkspaceSource = fakeSource(),
  measure: () => WorkspaceRoom | undefined = () => roomyGrid,
  overviewFilter: RememberedFilter = keptFilter(),
) {
  let next = 0
  return makeDesktopStore({
    workspace: source,
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure,
    overviewFilter,
  })
}

/** Lets every settled promise run its continuations. */
export async function settle(times = 5): Promise<void> {
  for (let index = 0; index < times; index++) await Promise.resolve()
}
