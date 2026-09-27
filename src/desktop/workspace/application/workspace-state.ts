/**
 * The workspace as the window holds it: the source's organisation and the
 * transcripts it has loaded, the new sessions not yet started, where each
 * conversation sits, and the window's own arrangement — which columns are
 * open, what the session list shows, which parts of the sidebar are folded.
 *
 * The source owns sessions; this is a projection of them plus what only the
 * window decides. Use cases in `usecases/` are pure functions over it, and
 * the store in `adapters/store/` applies them.
 */
import type { Channel, ModelRef, Section, SessionSummary } from "../model/organisation"
import { focusedPane, panesOf, type PaneLayout } from "../model/pane-layout"
import type { SessionView } from "../model/session-groups"
import { keepShownDrafts, type Draft } from "../model/session-lifecycle"
import { unconfirmed, type Message, type Transcript } from "../model/transcript"

/** An answer given to one approval: on its way, or refused with the reason. */
export interface Answer {
  readonly approvalId: string
  /** This answer's own, so only its refusal — not an earlier one's — sets it aside. */
  readonly token: string
  readonly failure?: string
}

export interface Chrome {
  readonly sidebarOpen: boolean
  readonly sessionListOpen: boolean
  /** A width the person dragged the sidebar to; each layout has its own until then. */
  readonly sidebarWidth: number | null
  readonly sessionListWidth: number
}

/** What is folded in the sidebar. */
export interface Tree {
  readonly collapsedSections: readonly string[]
  /** Channels disclosing their sessions, in the sidebar that shows them inline. */
  readonly expandedChannels: readonly string[]
  /** Channels showing every session rather than the newest few. */
  readonly showAllChannels: readonly string[]
}

export type LoadStatus = "loading" | "ready" | "failed"

export interface WorkspaceState {
  readonly status: LoadStatus
  /** Why the organisation could not be read, when it could not. */
  readonly failure: string | null
  readonly sections: readonly Section[]
  readonly channels: readonly Channel[]
  readonly sessions: Readonly<Record<string, SessionSummary>>
  readonly drafts: Readonly<Record<string, Draft>>
  /** The conversations as the source last gave them, by session. */
  readonly transcripts: Readonly<Record<string, Transcript>>
  /**
   * Messages the person sent that the source's conversation does not hold yet,
   * by session: sending, sent, or refused. Shown after the conversation until
   * it includes them; kept apart so no replacement from the source can lose one.
   */
  readonly outbox: Readonly<Record<string, readonly Message[]>>
  /** Why a conversation a pane shows could not be read, by session. */
  readonly transcriptFailures: Readonly<Record<string, string>>
  /**
   * Sessions the source removed, with the revision it removed them at, so an
   * older summary — an organisation read before the removal, say — cannot
   * bring one back.
   */
  readonly removed: Readonly<Record<string, number>>
  /**
   * The person's answer to a session's approval, by session: on its way, or
   * refused with a reason. It concerns only the approval it names; once the
   * source's conversation moves past that approval, it goes.
   */
  readonly answers: Readonly<Record<string, Answer>>
  /** The model the person chose for a session's next message, by session. */
  readonly chosenModels: Readonly<Record<string, ModelRef>>
  /** Where conversations sit; none until the organisation arrives. */
  readonly panes: PaneLayout | null
  readonly view: SessionView
  readonly chrome: Chrome
  readonly tree: Tree
}

export const initialChrome: Chrome = {
  sidebarOpen: true,
  sessionListOpen: true,
  sidebarWidth: null,
  sessionListWidth: 312,
}

export const initialWorkspace: WorkspaceState = {
  status: "loading",
  failure: null,
  sections: [],
  channels: [],
  sessions: {},
  drafts: {},
  transcripts: {},
  outbox: {},
  transcriptFailures: {},
  removed: {},
  answers: {},
  chosenModels: {},
  panes: null,
  view: { kind: "channel", channelId: "" },
  chrome: initialChrome,
  tree: { collapsedSections: [], expandedChannels: [], showAllChannels: [] },
}

/**
 * Reads a record keyed by an id from outside this module — a session id from
 * the source, a drop, an agent — for what it holds and nothing it inherits.
 */
export function entry<T>(
  record: Readonly<Record<string, T>>,
  key: string,
): T | undefined {
  return Object.hasOwn(record, key) ? record[key] : undefined
}

export function sessionOf(
  state: WorkspaceState,
  sessionId: string,
): SessionSummary | undefined {
  return entry(state.sessions, sessionId)
}

export function draftOf(state: WorkspaceState, sessionId: string): Draft | undefined {
  return entry(state.drafts, sessionId)
}

export function channelOf(state: WorkspaceState, channelId: string): Channel | undefined {
  return state.channels.find((channel) => channel.id === channelId)
}

/** Every session the source listed, drafts never among them. */
export function listedSessions(state: WorkspaceState): SessionSummary[] {
  return Object.values(state.sessions)
}

export function withSession(
  state: WorkspaceState,
  session: SessionSummary,
): WorkspaceState {
  if (entry(state.sessions, session.id) === session) return state
  return { ...state, sessions: { ...state.sessions, [session.id]: session } }
}

/**
 * Holds the source's conversation for a session, retiring from the outbox
 * every message it now includes, and any answer to an approval it no longer
 * asks.
 */
export function withTranscript(
  state: WorkspaceState,
  transcript: Transcript,
): WorkspaceState {
  const id = transcript.sessionId
  if (entry(state.transcripts, id) === transcript) return state
  const sent = entry(state.outbox, id) ?? []
  const waiting = unconfirmed(sent, transcript)
  const answer = entry(state.answers, id)
  return {
    ...state,
    transcripts: { ...state.transcripts, [id]: transcript },
    // Held now, so a read that failed before no longer matters.
    transcriptFailures: without(state.transcriptFailures, id),
    outbox:
      waiting === sent
        ? state.outbox
        : waiting.length > 0
          ? { ...state.outbox, [id]: waiting }
          : without(state.outbox, id),
    // An answer to an approval the conversation no longer asks has settled.
    answers:
      answer && transcript.approval?.id !== answer.approvalId
        ? without(state.answers, id)
        : state.answers,
  }
}

/** A record without `key`; the same record when it has no such key. */
export function without<T>(
  record: Readonly<Record<string, T>>,
  key: string,
): Readonly<Record<string, T>> {
  if (!Object.hasOwn(record, key)) return record
  const rest = { ...record }
  delete rest[key]
  return rest
}

/**
 * Applies a change of panes, letting go of any draft no pane shows any more.
 * The one place panes change, so no draft outlives its pane.
 */
export function withPanes(state: WorkspaceState, panes: PaneLayout): WorkspaceState {
  if (panes === state.panes) return state
  const shown = new Set(panesOf(panes).map((pane) => pane.sessionId))
  // A failed read belongs to the pane that showed it: shown again, a session is read afresh.
  const failed = Object.keys(state.transcriptFailures)
  const transcriptFailures = failed.every((id) => shown.has(id))
    ? state.transcriptFailures
    : Object.fromEntries(
        Object.entries(state.transcriptFailures).filter(([id]) => shown.has(id)),
      )
  return {
    ...state,
    panes,
    drafts: keepShownDrafts(state.drafts, shown),
    transcriptFailures,
  }
}

/** Adds `id` to a list of ids, or takes it out; the same list when nothing changes. */
export function toggled(
  ids: readonly string[],
  id: string,
  include: boolean,
): readonly string[] {
  const has = ids.includes(id)
  if (has === include) return ids
  return include ? [...ids, id] : ids.filter((other) => other !== id)
}

/** The channel of the session in the focused pane, draft or not. */
export function focusedChannel(state: WorkspaceState): string | undefined {
  if (!state.panes) return undefined
  const sessionId = focusedPane(state.panes).sessionId
  return sessionOf(state, sessionId)?.channelId ?? draftOf(state, sessionId)?.channelId
}

/** The model a session's next message is sent with: the one chosen, else its own. */
export function modelForNextTurn(
  state: WorkspaceState,
  sessionId: string,
): ModelRef | undefined {
  return (
    draftOf(state, sessionId)?.model ??
    entry(state.chosenModels, sessionId) ??
    sessionOf(state, sessionId)?.model
  )
}

/**
 * Everything the window holds of a session, forgotten: its summary, its
 * conversation, a failed read, what waits beside it. Panes are the caller's.
 */
export function forgetSession(state: WorkspaceState, sessionId: string): WorkspaceState {
  return {
    ...state,
    sessions: without(state.sessions, sessionId),
    transcripts: without(state.transcripts, sessionId),
    transcriptFailures: without(state.transcriptFailures, sessionId),
    outbox: without(state.outbox, sessionId),
    answers: without(state.answers, sessionId),
    chosenModels: without(state.chosenModels, sessionId),
  }
}

/** Whether an answer to this approval is on its way: given, and not refused. */
export function answering(answer: Answer | undefined, approvalId: string): boolean {
  return answer?.approvalId === approvalId && answer.failure === undefined
}
