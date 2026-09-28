/**
 * The workspace as the window holds it: the source's index and the
 * transcripts it has loaded, the new sessions not yet started, where each
 * conversation sits, and the window's own arrangement — which columns are
 * open, what the session list shows, which parts of the sidebar are folded.
 *
 * The source owns sessions; this is a projection of them plus what only the
 * window decides. Use cases in `usecases/` are pure functions over it, and
 * the store in `adapters/store/` applies them.
 */
import type { WorkspaceFailureReason } from "../model/failure"
import type { Channel, ModelRef, Section, SessionSummary } from "../model/workspace-index"
import { focusedPane, panesOf, paneShowing, type PaneLayout } from "../model/pane-layout"
import { keptConversations, retention, type Removal } from "../model/retention"
import { defaultFilter, listsWaiting, type AgentsFilter } from "../model/overview/filter"
import { drawn, type SideColumn } from "../../model/side-column"
import type { SessionView } from "../model/session-groups"
import { keepShownDrafts, type Draft } from "../model/session-lifecycle"
import { unconfirmed, type Message, type Transcript } from "../model/transcript"

/** An answer given to one approval: on its way, or refused with the reason. */
export interface Answer {
  readonly approvalId: string
  /** This answer's own, so only its refusal — not an earlier one's — sets it aside. */
  readonly token: string
  readonly failure?: WorkspaceFailureReason
}

/**
 * The side columns, each shown or hidden by the person's choice and folded
 * for room by the window (`SideColumn`), so a fold lifts on its own once
 * there is room again (`usecases/navigation.ts`).
 */
export interface Chrome {
  readonly sidebar: SideColumn
  readonly sessionList: SideColumn
  /** A width the person dragged the sidebar to; each layout has its own until then. */
  readonly sidebarWidth: number | null
  readonly sessionListWidth: number
}

/** Which side columns are drawn: open by the person's choice, and not folded for room. */
export function drawnColumns(chrome: Chrome): { sidebar: boolean; sessionList: boolean } {
  return { sidebar: drawn(chrome.sidebar), sessionList: drawn(chrome.sessionList) }
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
  /** Why the index could not be read, when it could not. */
  readonly failure: WorkspaceFailureReason | null
  /**
   * While a read of the index is on its way: how many, and the sessions
   * the stream brought meanwhile, which the read may predate — a session it
   * does not list is taken out unless it is one of these. `null` when no read
   * is on its way.
   */
  readonly reading: { readonly reads: number; readonly heard: readonly string[] } | null
  readonly sections: readonly Section[]
  readonly channels: readonly Channel[]
  readonly sessions: Readonly<Record<string, SessionSummary>>
  readonly drafts: Readonly<Record<string, Draft>>
  /**
   * The conversations as the source last gave them, by session: every one a
   * pane shows, and a few others (`model/retention.ts`).
   */
  readonly transcripts: Readonly<Record<string, Transcript>>
  /**
   * Messages the person sent that the source's conversation does not hold yet,
   * by session: sending, sent, or refused. Shown after the conversation until
   * it includes them; kept apart so no replacement from the source can lose one.
   */
  readonly outbox: Readonly<Record<string, readonly Message[]>>
  /** Why a conversation a pane shows could not be read, by session. */
  readonly transcriptFailures: Readonly<Record<string, WorkspaceFailureReason>>
  /**
   * Sessions the source removed, newest last, with the summary revision it
   * removed them at, so an older summary — an index read before the
   * removal, say — cannot bring one back. Bounded (`model/retention.ts`).
   */
  readonly removed: readonly Removal[]
  /**
   * The person's answer to a session's approval, by session: on its way, or
   * refused with a reason. It concerns only the approval it names; once the
   * source's conversation moves past that approval, it goes.
   */
  readonly answers: Readonly<Record<string, Answer>>
  /** The model the person chose for a session's next message, by session. */
  readonly chosenModels: Readonly<Record<string, ModelRef>>
  /**
   * What the person has typed in a session's composer and not sent, by
   * session or new session: the window's own, kept here so it survives a
   * change of layout or a pane showing another session, and so an agent can
   * write it (`setComposerText`). Sent (`messageSent`, whoever sent it), or its
   * session let go, it goes.
   */
  readonly composerText: Readonly<Record<string, string>>
  /** Where conversations sit; none until the index arrives. */
  readonly panes: PaneLayout | null
  readonly view: SessionView
  /**
   * What fills the content region: the panes, or the Agents overview over
   * them — the overview's open state. Going anywhere else, or changing the
   * panes, is going back to the panes (`usecases/navigation.ts`,
   * `navigated`).
   */
  readonly content: ContentView
  /** What the Agents overview shows while open: its filter and the session chosen. */
  readonly overview: OverviewState
  readonly chrome: Chrome
  readonly tree: Tree
}

/** The content region's view: the chat panes, or every agent at a glance. */
export type ContentView = "panes" | "agents"

/** The Agents overview's own state: kept while it is closed, for when it opens again. */
export interface OverviewState {
  /** The session the peek shows, chosen in the list; kept even when a filter hides it. */
  readonly selected: string | null
  /** What it lists (`model/overview/filter.ts`), remembered between launches. */
  readonly filter: AgentsFilter
}

export const initialWorkspace: WorkspaceState = {
  status: "loading",
  failure: null,
  reading: null,
  sections: [],
  channels: [],
  sessions: {},
  drafts: {},
  transcripts: {},
  outbox: {},
  transcriptFailures: {},
  removed: [],
  answers: {},
  chosenModels: {},
  composerText: {},
  panes: null,
  view: { kind: "channel", channelId: "" },
  content: "panes",
  overview: { selected: null, filter: defaultFilter },
  chrome: {
    sidebar: { open: true, folded: false },
    sessionList: { open: true, folded: false },
    sidebarWidth: null,
    sessionListWidth: 312,
  },
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

/** The sessions the panes show, drafts among them. */
function paneIds(panes: PaneLayout | null): string[] {
  return panes ? panesOf(panes).map((pane) => pane.sessionId) : []
}

/**
 * Whether the open overview shows a session: the one chosen in it, or one
 * its filter lists waiting on the person (`listsWaiting`). The one rule for
 * what the overview puts on screen — what it answers (`onScreen`), and what
 * the window reads and keeps for it (`shownIds`).
 */
export function overviewShows(state: WorkspaceState, sessionId: string): boolean {
  if (state.content !== "agents") return false
  if (state.overview.selected === sessionId) return true
  const session = sessionOf(state, sessionId)
  return session !== undefined && listsWaiting(session, state.overview.filter)
}

/**
 * On screen: in a pane, or in the open overview. Only an approval on screen
 * is answered (`approve`, `deny`), so an agent answers what it has opened.
 */
export function onScreen(state: WorkspaceState, sessionId: string): boolean {
  return (
    (state.panes !== null && paneShowing(state.panes, sessionId) !== undefined) ||
    overviewShows(state, sessionId)
  )
}

/**
 * The sessions the open overview shows whose conversations the window
 * reads and keeps: the one chosen, then the waiting ones, most recently
 * active first, at most `retention.overviewConversations`. Past the bound, a
 * waiting session's conversation is read once its row is chosen.
 */
export function overviewShownIds(state: WorkspaceState): string[] {
  if (state.content !== "agents") return []
  const { selected } = state.overview
  const waiting = listedSessions(state)
    .filter((session) => session.id !== selected && overviewShows(state, session.id))
    .sort((a, b) => b.updatedAt - a.updatedAt || (a.id < b.id ? -1 : 1))
    .map((session) => session.id)
  const chosen = selected !== null && sessionOf(state, selected) ? [selected] : []
  return [...chosen, ...waiting].slice(0, retention.overviewConversations)
}

/**
 * The sessions on screen whose conversations the window keeps whatever
 * their age: the panes' (drafts among them), and the overview's, bounded.
 */
export function shownIds(state: WorkspaceState): Set<string> {
  return new Set([...paneIds(state.panes), ...overviewShownIds(state)])
}

/**
 * Keeps a failed read only while its session is on screen: shown again, a
 * session is read afresh. The same state when nothing is let go.
 */
export function keepShownFailures(state: WorkspaceState): WorkspaceState {
  const failed = Object.keys(state.transcriptFailures)
  if (failed.length === 0) return state
  const shown = shownIds(state)
  if (failed.every((id) => shown.has(id))) return state
  return {
    ...state,
    transcriptFailures: Object.fromEntries(
      Object.entries(state.transcriptFailures).filter(([id]) => shown.has(id)),
    ),
  }
}

/**
 * Holds the source's conversation for a session, retiring from the outbox
 * every message it now includes, and any answer to an approval it no longer
 * asks. Past what is kept of sessions no pane shows, the least recently
 * active of those is let go (`model/retention.ts`).
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
  const transcripts = keptConversations(
    { ...state.transcripts, [id]: transcript },
    shownIds(state),
    (sessionId) => sessionOf(state, sessionId)?.updatedAt ?? 0,
  )
  return {
    ...state,
    transcripts,
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
  const shown = new Set(paneIds(panes))
  const drafts = keepShownDrafts(state.drafts, shown)
  // A new session's typing goes with it; a listed session's stays until it is let go.
  const composerText =
    drafts === state.drafts
      ? state.composerText
      : Object.keys(state.drafts)
          .filter((id) => !Object.hasOwn(drafts, id))
          .reduce(without, state.composerText)
  // A failed read belongs to what showed it: shown again, a session is read afresh.
  return keepShownFailures({ ...state, panes, drafts, composerText })
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
    composerText: without(state.composerText, sessionId),
    overview:
      state.overview.selected === sessionId
        ? { ...state.overview, selected: null }
        : state.overview,
  }
}

/** Whether an answer to this approval is on its way: given, and not refused. */
export function answering(answer: Answer | undefined, approvalId: string): boolean {
  return answer?.approvalId === approvalId && answer.failure === undefined
}
