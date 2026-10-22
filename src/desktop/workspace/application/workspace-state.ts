import type { ModelChoice } from "../model/model-choice"
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
import type { StageMismatch, WorkspaceFailureReason } from "../model/failure"
import { knownToSource } from "../model/revision"
import type { Channel, ModelRef, Section, SessionSummary } from "../model/workspace-index"
import {
  focusedPane,
  panesOf,
  paneShowing,
  type Pane,
  type PaneLayout,
} from "../../split-panes/model/pane-layout"
import { paneItemKey, paneItemOf, sessionItem, type PaneItem } from "../model/pane-item"
import { sameWidget, type WidgetRef } from "../../widgets/model/widget-ref"
import { keptConversations, retention, type Removal } from "../model/retention"
import { defaultFilter, listsWaiting, type AgentsFilter } from "../model/overview/filter"
import { inGroup, type AgentsGroup } from "../model/overview/agents-glance"
import { drawn, type SideColumn } from "../../model/side-column"
import type { WindowArrangement } from "../model/shown-window"
import type { SessionView } from "../model/session-groups"
import { keepShownDrafts, type Draft } from "../model/session-lifecycle"
import { unconfirmed, type Transcript, type TurnMessage } from "../model/transcript"
import { settledBy, type AccessChoice } from "../model/access"
import type {
  ShareDraft,
  ShareGrant,
  ShareRequest,
  SharingChoices,
  SharingFailure,
} from "../model/sharing"

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

/**
 * Whether the workspace lists a conversation: unread until the index is
 * read (`status === "ready"`; a failed index stays unread), then listed or
 * absent. The subagents panel's answer reads this (`selectSessionListing`).
 */
export type SessionListing = "unread" | "listed" | "absent"

/** A read of the index on its way, and the sessions the stream brought since it was asked. */
export interface IndexRead {
  readonly read: string
  readonly heard: readonly string[]
  /**
   * A read asked after this one has answered and been applied: this one's
   * answer, whenever it comes, is older than what the window holds, and is
   * let go unread.
   */
  readonly outrun: boolean
}

export interface WorkspaceState {
  readonly status: LoadStatus
  /** Why the index could not be read, when it could not. */
  readonly failure: WorkspaceFailureReason | null
  /**
   * Both stages when `failure` is `wrong-stage`. Absent for every other
   * reason, including after a later read clears the failure.
   */
  readonly failureStages: StageMismatch | null
  /**
   * The reads of the index on their way, each by the name it was asked
   * under, with the sessions the stream brought since that read was asked —
   * which the read may predate. A session a read does not list is taken out
   * unless the stream brought it since that same read was asked: what came
   * before it, the read is the newer word on. Empty when none is on its way.
   */
  readonly reading: readonly IndexRead[]
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
   * The newest conversation revision held, by listed session — kept when its
   * content is let go (`model/retention.ts`), so a read answered after that
   * with an older conversation cannot bring the old one back. Let go with the
   * session.
   */
  readonly conversationRevisions: Readonly<Record<string, number>>
  /**
   * Messages the person sent that the source's conversation does not hold yet,
   * by session: sending, sent, or refused. Shown after the conversation until
   * it includes them; kept apart so no replacement from the source can lose one.
   */
  readonly outbox: Readonly<Record<string, readonly TurnMessage[]>>
  /** Why a conversation a pane shows could not be read, by session. */
  readonly transcriptFailures: Readonly<Record<string, WorkspaceFailureReason>>
  /**
   * Sessions the source removed, newest last, with the summary revision it
   * removed them at, so an older summary — an index read before the
   * removal, say — cannot bring one back. Bounded (`model/retention.ts`).
   */
  readonly removed: readonly Removal[]
  /**
   * The person's answers to a session's approvals, by session, one for each
   * approval answered: on its way, or refused with a reason. Each concerns
   * only the approval it names; once the source's conversation no longer
   * asks that approval, it goes, and the others stay.
   */
  readonly answers: Readonly<Record<string, readonly Answer[]>>
  /** The model the person chose for a session's next message, by session. */
  readonly modelChoices: Readonly<Record<string, ModelChoice>>
  readonly chosenModels: Readonly<Record<string, ModelRef>>
  /**
   * The model the person picked in an established session's composer, by
   * session: only a choice until the next message is sent, which asks the
   * source for it first (`model-commands.ts`), or asks nothing when the
   * session settled on it. Only that send settles it, so a summary passing
   * through the same model, from this window or another client, does not.
   */
  readonly stagedModels: Readonly<Record<string, ModelRef>>
  /**
   * The access the person chose for a session, by session or new session,
   * until the source's conversation says it (`model/access.ts`). The
   * source's own word is the transcript's `access`.
   */
  readonly accessChoices: Readonly<Record<string, AccessChoice>>
  /**
   * What the person has typed in a session's composer and not sent, by
   * session or new session: the window's own, kept here so it survives a
   * change of layout or a pane showing another session, and so an agent can
   * write it (`setComposerText`). Sent by the person (`messageSent`), or its
   * session let go, it goes; an agent's message leaves it as it is.
   */
  readonly composerText: Readonly<Record<string, string>>
  /** Where conversations sit; none until the index arrives. */
  readonly panes: PaneLayout | null
  readonly view: SessionView
  /**
   * What fills the content region: the panes, the Agents overview over
   * them — the overview's open state — or a widget over them (the window
   * place, ADR 326). Going anywhere else, or changing the panes, is going
   * back to the panes (`usecases/navigation.ts`, `navigated`).
   */
  readonly content: ContentView
  /** What the Agents overview shows while open: its filter, the group shown alone, and the session chosen. */
  readonly overview: OverviewState
  readonly chrome: Chrome
  /**
   * The layout drawing the panes now, as the layout itself says when it is
   * on screen (`arrangementShown`); `null` while none is, as in the classic
   * shell. What the window tells the person's agents it shows
   * (`usecases/window.ts`) is said only while one is.
   */
  readonly arrangement: WindowArrangement | null
  readonly tree: Tree
  /**
   * Whether a session can be shared from this window: composition gave it a
   * sharing source. Read once, into the first state, as the overview's
   * filter is.
   */
  readonly shareable: boolean
  /** The share dialog, while it is open (`usecases/sharing.ts`). */
  readonly share: ShareDialog | null
  /**
   * The title being renamed where it stands, and what is typed so far
   * (`usecases/row-editing.ts`). The window's, not the row's: a row is drawn
   * afresh when its session moves group, and the edit outlives that.
   */
  readonly renaming: { readonly sessionId: string; readonly draft: string } | null
  /** The session whose deletion is being asked, while the question is open. */
  readonly deleteAsked: string | null
  /**
   * The one share or revoke on its way to the gateway, and the session it
   * changes. It belongs to the window, not to a dialog: closing the dialog
   * does not stop it, and no other change begins, from any dialog, until it
   * settles (D6, D7).
   */
  readonly shareChange: {
    readonly sessionId: string
    readonly change: ShareChange
  } | null
}

/**
 * The share dialog of one session (nessa_ui#128, #129), named by the
 * opening that made it: a read for any other opening is let go.
 */
export interface ShareDialog {
  readonly sessionId: string
  readonly opening: string
  /** The grants the gateway last listed; `null` until a read of them answers. */
  readonly grants: readonly ShareGrant[] | null
  /** What the form offers, as last read; `null` until a read of it answers. */
  readonly choices: SharingChoices | null
  /**
   * The read on its way, by name; only the newest is applied. While one is
   * on its way the form, Share and Revoke are held (D10).
   */
  readonly reading: string | null
  /** Which part of the newest read failed, and why, until another answers. */
  readonly readFailure: ShareReadFailure | null
  /** How the last change to this session settled, until the next begins. */
  readonly settled: ShareSettled | null
  /**
   * The Revoke the person is being asked to confirm: the device, and the
   * revision its grant was listed at when they were asked, which is what the
   * revoke names. Any change beginning ends it, and so does a read listing
   * that device at another revision, or not at all (D11).
   */
  readonly confirming: ShareConfirming | null
  /** The grant the person is making or changing. */
  readonly draft: ShareDraft
}

/**
 * A part of a read that failed: the grants, or what the form offers. The
 * grants' failure is said when both failed.
 */
export interface ShareReadFailure {
  readonly part: "grants" | "choices"
  readonly reason: SharingFailure
}

/** A Revoke being confirmed: the device, and its grant's revision when asked. */
export interface ShareConfirming {
  readonly credentialId: string
  readonly policyRevision: number
}

/** A change, and how it settled. */
export interface ShareSettled {
  readonly change: ShareChange
  readonly outcome: "done" | SharingFailure
  /**
   * Whether a read of the grants has answered since it settled. Until one
   * has, the grants listed predate the change, so a draft built on its
   * answer is not compared with them (D10).
   */
  readonly listed: boolean
}

/** A change to who a session is shared with: a grant to make, or one to take away. */
export type ShareChange =
  | { readonly kind: "share"; readonly request: ShareRequest }
  | {
      readonly kind: "revoke"
      readonly credentialId: string
      /** The revision of the grant as listed when Revoke was chosen. */
      readonly expectedPolicyRevision: number
    }

/**
 * The content region's view: the chat panes; every agent at a glance, over
 * the session list and the panes; or one widget over the panes alone, the
 * session list in reach beside it. The panes stay as they were beneath
 * either, so a widget can be in a pane and over the panes at once.
 */
export type ContentView = "panes" | "agents" | { readonly widget: WidgetRef }

/** What fills the content region, by kind: the panes, the overview, or a widget in the window. */
export type ContentKind = "panes" | "agents" | "widget"

/** The content view's kind: the one reading of its shape, with `windowWidget`. */
export function contentKind(content: ContentView): ContentKind {
  return typeof content === "object" ? "widget" : content
}

/** The widget the window shows over the panes, if the content view is one. */
export function windowWidget(content: ContentView): WidgetRef | null {
  return typeof content === "object" ? content.widget : null
}

/** Whether two content views are the same place: a widget compared by value (`sameWidget`). */
export function sameContent(a: ContentView, b: ContentView): boolean {
  if (typeof a === "string" || typeof b === "string") return a === b
  return sameWidget(a.widget, b.widget)
}

/** The Agents overview's own state: kept while it is closed, for when it opens again. */
export interface OverviewState {
  /** The session the peek shows, chosen in the list; kept even when a filter hides it. */
  readonly selected: string | null
  /** What it lists (`model/overview/filter.ts`), remembered between launches. */
  readonly filter: AgentsFilter
  /**
   * The one group it shows alone, chosen by its count in the header; `null`
   * shows every group. Kept while the window lives, as the chosen session
   * is, and never between launches: a window opens on every group, so a
   * request waiting on the person is never hidden behind yesterday's choice.
   */
  readonly group: AgentsGroup | null
}

export const initialWorkspace: WorkspaceState = {
  status: "loading",
  failure: null,
  failureStages: null,
  reading: [],
  sections: [],
  channels: [],
  sessions: {},
  drafts: {},
  transcripts: {},
  conversationRevisions: {},
  outbox: {},
  transcriptFailures: {},
  removed: [],
  answers: {},
  chosenModels: {},
  stagedModels: {},
  modelChoices: {},
  accessChoices: {},
  composerText: {},
  panes: null,
  view: { channelId: "" },
  content: "panes",
  overview: { selected: null, filter: defaultFilter, group: null },
  chrome: {
    sidebar: { open: true, folded: false },
    sessionList: { open: true, folded: false },
    sidebarWidth: null,
    sessionListWidth: 312,
  },
  arrangement: null,
  tree: { collapsedSections: [], expandedChannels: [], showAllChannels: [] },
  shareable: false,
  share: null,
  renaming: null,
  deleteAsked: null,
  shareChange: null,
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

/** What a pane shows, read through the one codec (`model/pane-item.ts`). */
export function itemIn(pane: Pane): PaneItem | null {
  return paneItemOf(pane.item)
}

/** The session a pane shows, draft or not; none when it shows a widget. */
export function sessionIn(pane: Pane): string | undefined {
  const item = itemIn(pane)
  return item?.kind === "session" ? item.sessionId : undefined
}

/** The widget a pane shows; none when it shows a session. */
export function widgetIn(pane: Pane): WidgetRef | undefined {
  const item = itemIn(pane)
  return item?.kind === "widget" ? item.widget : undefined
}

/** The pane showing `item`, if one does. */
export function paneShowingItem(panes: PaneLayout, item: PaneItem): Pane | undefined {
  return paneShowing(panes, paneItemKey(item))
}

/** The pane showing a session, draft or not, if one does. */
export function paneShowingSession(
  panes: PaneLayout,
  sessionId: string,
): Pane | undefined {
  return paneShowingItem(panes, sessionItem(sessionId))
}

/** The sessions the panes show, in reading order, drafts among them; a widget's pane shows none. */
export function paneIds(panes: PaneLayout | null): string[] {
  return panes ? panesOf(panes).flatMap((pane) => sessionIn(pane) ?? []) : []
}

/**
 * Whether the open overview shows a session: the one chosen in it, or one
 * its filter lists waiting on the person (`listsWaiting`) while its waiting
 * are shown — every group, or Needs you alone (`inGroup`). The one rule for
 * what the overview puts on screen — what it answers (`onScreen`), and what
 * the window reads and keeps for it (`shownIds`).
 */
export function overviewShows(state: WorkspaceState, sessionId: string): boolean {
  if (state.content !== "agents") return false
  if (state.overview.selected === sessionId) return true
  const session = sessionOf(state, sessionId)
  return (
    session !== undefined &&
    listsWaiting(session, state.overview.filter) &&
    inGroup(session, state.overview.group)
  )
}

/**
 * On screen: in a pane, or in the open overview. Only an approval on screen
 * is answered (`approve`, `deny`), so an agent answers what it has opened.
 */
export function onScreen(state: WorkspaceState, sessionId: string): boolean {
  return (
    (state.panes !== null && paneShowingSession(state.panes, sessionId) !== undefined) ||
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
  const choice = entry(state.accessChoices, id)
  const access = settledBy(choice, transcript)
  const transcripts = keptConversations(
    { ...state.transcripts, [id]: transcript },
    shownIds(state),
    (sessionId) => sessionOf(state, sessionId)?.updatedAt ?? 0,
  )
  return {
    ...state,
    transcripts,
    conversationRevisions: { ...state.conversationRevisions, [id]: transcript.revision },
    // Held now, so a read that failed before no longer matters.
    transcriptFailures: without(state.transcriptFailures, id),
    outbox:
      waiting === sent
        ? state.outbox
        : waiting.length > 0
          ? { ...state.outbox, [id]: waiting }
          : without(state.outbox, id),
    // An answer to an approval the conversation no longer asks has settled.
    answers: settledAnswers(state.answers, id, answer, transcript),
    // A choice of access the conversation now says has settled.
    accessChoices:
      access === choice
        ? state.accessChoices
        : access
          ? { ...state.accessChoices, [id]: access }
          : without(state.accessChoices, id),
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
  // A new session's typing, and its access, go with it; a listed session's
  // stay until it is let go.
  const dropped =
    drafts === state.drafts
      ? []
      : Object.keys(state.drafts).filter((id) => !Object.hasOwn(drafts, id))
  const composerText = dropped.reduce(without, state.composerText)
  const accessChoices = dropped.reduce(without, state.accessChoices)
  // A failed read belongs to what showed it: shown again, a session is read afresh.
  return keepShownFailures({ ...state, panes, drafts, composerText, accessChoices })
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

/** The channel of the session in the focused pane, draft or not; none when it shows a widget. */
export function focusedChannel(state: WorkspaceState): string | undefined {
  if (!state.panes) return undefined
  const sessionId = sessionIn(focusedPane(state.panes))
  if (sessionId === undefined) return undefined
  return sessionOf(state, sessionId)?.channelId ?? draftOf(state, sessionId)?.channelId
}

/** Drafts choose locally; established source sessions stage a pick their next send applies. */
export function canChooseModel(state: WorkspaceState, sessionId: string): boolean {
  const session = sessionOf(state, sessionId)
  return !!draftOf(state, sessionId) || (!!session && knownToSource(session))
}

/**
 * The model a session's next message is sent with. Draft choices do not
 * override the source-owned model of an established session; only a pick
 * staged in its composer does, and sending asks the source for it first.
 */
export function modelForNextTurn(
  state: WorkspaceState,
  sessionId: string,
): ModelRef | undefined {
  const session = sessionOf(state, sessionId)
  if (session && knownToSource(session))
    return entry(state.stagedModels, sessionId) ?? session.model
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
    conversationRevisions: without(state.conversationRevisions, sessionId),
    transcriptFailures: without(state.transcriptFailures, sessionId),
    outbox: without(state.outbox, sessionId),
    answers: without(state.answers, sessionId),
    chosenModels: without(state.chosenModels, sessionId),
    stagedModels: without(state.stagedModels, sessionId),
    accessChoices: without(state.accessChoices, sessionId),
    modelChoices: without(state.modelChoices, sessionId),
    composerText: without(state.composerText, sessionId),
    overview:
      state.overview.selected === sessionId
        ? { ...state.overview, selected: null }
        : state.overview,
    // An edit or a question about a session gone ends with it, and does not
    // come back if the session does.
    renaming: state.renaming?.sessionId === sessionId ? null : state.renaming,
    deleteAsked: state.deleteAsked === sessionId ? null : state.deleteAsked,
  }
}

/**
 * A session's answers without those to approvals its conversation no longer
 * asks; the same record when none settled.
 */
function settledAnswers(
  answers: WorkspaceState["answers"],
  id: string,
  given: readonly Answer[] | undefined,
  transcript: Transcript,
): WorkspaceState["answers"] {
  if (!given) return answers
  const asked = new Set(transcript.approvals.map((approval) => approval.id))
  const open = given.filter((answer) => asked.has(answer.approvalId))
  if (open.length === given.length) return answers
  return open.length > 0 ? { ...answers, [id]: open } : without(answers, id)
}

/** The person's answer to one approval of a session, if they gave one. */
export function answerOf(
  state: WorkspaceState,
  sessionId: string,
  approvalId: string,
): Answer | undefined {
  return entry(state.answers, sessionId)?.find(
    (answer) => answer.approvalId === approvalId,
  )
}

/** Whether an answer to this approval is on its way: given, and not refused. */
export function answering(answer: Answer | undefined, approvalId: string): boolean {
  return answer?.approvalId === approvalId && answer.failure === undefined
}
