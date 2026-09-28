/**
 * Reads of the workspace for the views, each as narrow as the view that uses
 * it: a pane reads its own placement and session, a row reads its own
 * session, a channel reads its own activity. Derived lists are memoised on
 * the parts of state they come from, so a streamed word — which replaces one
 * transcript — changes no selector but that transcript's.
 *
 * No view selects the workspace as a whole; that would render the whole
 * window for every change.
 */
import { createSelector } from "@reduxjs/toolkit"
import {
  byRecency,
  type Channel,
  type ModelRef,
  type SessionSummary,
} from "../../model/workspace-index"
import {
  focusedPane,
  paneByKey,
  paneCount,
  panesOf,
  type PaneKey,
} from "../../model/pane-layout"
import {
  placements,
  type EdgePlacement,
  type PanePlacement,
} from "../../model/pane-sizing"
import {
  branchSessions,
  channelActivity,
  groupByStatus,
  inView,
  matchesSearch,
  statusCounts,
  type ChannelActivity,
  type SessionView,
} from "../../model/session-groups"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { Draft } from "../../model/session-lifecycle"
import type { Message, Transcript } from "../../model/transcript"
import {
  channelOf,
  drawnColumns,
  entry,
  modelForNextTurn,
  focusedChannel,
  type Answer,
  type Chrome,
  type WorkspaceState,
} from "../../application/workspace-state"

type Root = { workspace: WorkspaceState }

export const selectFailure = (state: Root) => state.workspace.failure
export const selectChrome = (state: Root): Chrome => state.workspace.chrome
/** Whether the sidebar is drawn: the person's choice, unless the window folded it for room. */
export const selectSidebarOpen = (state: Root) =>
  drawnColumns(state.workspace.chrome).sidebar
/** Whether the session list is drawn, as `selectSidebarOpen` says of the sidebar. */
export const selectSessionListOpen = (state: Root) =>
  drawnColumns(state.workspace.chrome).sessionList
/** Whether the person has the session list open, drawn or folded for room: Settings shows their choice. */
export const selectSessionListChosen = (state: Root) =>
  state.workspace.chrome.sessionList.open
/** How many columns of panes there are; the side columns are fitted around them. */
export const selectColumnCount = (state: Root) =>
  state.workspace.panes?.columns.length ?? 1
export const selectView = (state: Root): SessionView => state.workspace.view
/** What fills the content region: the panes, or the Agents overview. */
export const selectContentView = (state: Root) => state.workspace.content
export const selectPanes = (state: Root) => state.workspace.panes
export const selectSections = (state: Root) => state.workspace.sections
export const selectChannels = (state: Root) => state.workspace.channels
const selectSessions = (state: Root) => state.workspace.sessions

export const selectSession = (
  state: Root,
  sessionId: string,
): SessionSummary | undefined => entry(state.workspace.sessions, sessionId)
export const selectDraft = (state: Root, sessionId: string): Draft | undefined =>
  entry(state.workspace.drafts, sessionId)
export const selectTranscript = (
  state: Root,
  sessionId: string,
): Transcript | undefined => entry(state.workspace.transcripts, sessionId)
const nothingSent: readonly Message[] = []

/** What the person sent to a session that its conversation does not hold yet. */
export const selectOutbox = (state: Root, sessionId: string): readonly Message[] =>
  entry(state.workspace.outbox, sessionId) ?? nothingSent

/** What is typed in a session's composer and not sent. */
export const selectComposerText = (state: Root, sessionId: string): string =>
  entry(state.workspace.composerText, sessionId) ?? ""

/** The model a session's next message is sent with: chosen in its composer, else its own. */
export const selectNextModel = (state: Root, sessionId: string): ModelRef | undefined =>
  modelForNextTurn(state.workspace, sessionId)

/** The person's answer to a session's approval, while it is on its way or after it failed. */
export const selectAnswer = (
  state: Root,
  sessionId: string,
  approvalId: string,
): Answer | undefined => {
  const answer = entry(state.workspace.answers, sessionId)
  return answer?.approvalId === approvalId ? answer : undefined
}

export const selectTranscriptFailure = (
  state: Root,
  sessionId: string,
): WorkspaceFailureReason | undefined =>
  entry(state.workspace.transcriptFailures, sessionId)
export const selectChannel = (state: Root, channelId: string): Channel | undefined =>
  channelOf(state.workspace, channelId)

const listed = createSelector([selectSessions], (sessions) => Object.values(sessions))

const channelIdsBySection = createSelector([selectChannels], (channels) => {
  const bySection = new Map<string, string[]>()
  for (const channel of channels) {
    const ids = bySection.get(channel.sectionId)
    if (ids) ids.push(channel.id)
    else bySection.set(channel.sectionId, [channel.id])
  }
  return bySection
})

const noIds: readonly string[] = []

/** A section's channels, as ids, in the index's order. */
export const selectChannelIdsIn = (state: Root, sectionId: string): readonly string[] =>
  channelIdsBySection(state).get(sectionId) ?? noIds

/** The channel of the session in the focused pane. */
export const selectFocusedChannel = (state: Root): string | undefined =>
  focusedChannel(state.workspace)

const noPlacements = { panes: [] as PanePlacement[], edges: [] as EdgePlacement[] }

const selectColumns = (state: Root) => state.workspace.panes?.columns

/**
 * Where every pane and edge is drawn; recomputed only when the columns change,
 * not when focus moves, so focusing a pane renders the two panes it concerns.
 */
export const selectPlacements = createSelector([selectColumns], (columns) =>
  columns ? placements(columns) : noPlacements,
)

export const selectPaneCount = (state: Root) =>
  state.workspace.panes ? paneCount(state.workspace.panes) : 0

export const selectFocusedPaneKey = (state: Root): PaneKey | null =>
  state.workspace.panes ? focusedPane(state.workspace.panes).key : null

export const selectFocusedSessionId = (state: Root): string | null =>
  state.workspace.panes ? focusedPane(state.workspace.panes).sessionId : null

/** The session a pane shows. */
export const selectPaneSession = (state: Root, pane: PaneKey): string | null =>
  state.workspace.panes
    ? (paneByKey(state.workspace.panes, pane)?.sessionId ?? null)
    : null

/** The sessions on screen, in reading order. */
export const selectShownSessionIds = createSelector([selectPanes], (panes) =>
  panes ? panesOf(panes).map((pane) => pane.sessionId) : [],
)

export const selectStatusCounts = createSelector([listed], statusCounts)

/** A channel's sessions, newest-first order left to the caller. */
const sessionsByChannel = createSelector([listed], (sessions) => {
  const byChannel = new Map<string, SessionSummary[]>()
  for (const session of sessions) {
    const list = byChannel.get(session.channelId)
    if (list) list.push(session)
    else byChannel.set(session.channelId, [session])
  }
  return byChannel
})

const none: readonly SessionSummary[] = []
const channelSessions = (state: Root, channelId: string) =>
  sessionsByChannel(state).get(channelId) ?? none

/** What a channel's row shows about its sessions; compare with `shallowEqual`. */
export const selectChannelActivity = (state: Root, channelId: string): ChannelActivity =>
  channelActivity(channelSessions(state, channelId))

/** The pinned sessions a channel hangs beneath it, as ids; compare with `shallowEqual`. */
export const selectPinnedIds = (state: Root, channelId: string): string[] =>
  channelSessions(state, channelId)
    .filter((session) => session.pinned)
    .map((session) => session.id)

/** The sessions a channel's branch discloses, as ids, and how many it holds back. */
export function selectBranch(
  state: Root,
  channelId: string,
): { ids: string[]; hidden: number; total: number } {
  const sessions = channelSessions(state, channelId)
  const shown = new Set(selectShownSessionIds(state))
  const { visible, hidden } = branchSessions(sessions, {
    showAll: state.workspace.tree.showAllChannels.includes(channelId),
    shown,
  })
  return { ids: visible.map((session) => session.id), hidden, total: sessions.length }
}

/** Whether two branch readings disclose the same rows. */
export function sameBranch(
  a: ReturnType<typeof selectBranch>,
  b: ReturnType<typeof selectBranch>,
): boolean {
  return (
    a.hidden === b.hidden &&
    a.total === b.total &&
    a.ids.length === b.ids.length &&
    a.ids.every((id, index) => id === b.ids[index])
  )
}

export const selectChannelExpanded = (state: Root, channelId: string) =>
  state.workspace.tree.expandedChannels.includes(channelId)
export const selectShowAll = (state: Root, channelId: string) =>
  state.workspace.tree.showAllChannels.includes(channelId)
export const selectSectionCollapsed = (state: Root, sectionId: string) =>
  state.workspace.tree.collapsedSections.includes(sectionId)

export interface ListGroup {
  readonly id: string
  readonly label: string
  readonly ids: readonly string[]
}

/**
 * The session list's groups for the current view and a search, as ids;
 * running sessions kept in a group above the rest unless `runningFirst` is
 * off.
 */
export function selectListGroups(
  state: Root,
  query: string,
  runningFirst = true,
): ListGroup[] {
  const view = state.workspace.view
  return groupByStatus(
    listed(state).filter(
      (session) => inView(session, view) && matchesSearch(session, query),
    ),
    { runningFirst },
  ).map((group) => ({
    id: group.id,
    label: group.label,
    ids: group.sessions.map((session) => session.id),
  }))
}

/** Whether two list readings show the same rows in the same groups. */
export function sameListGroups(
  a: readonly ListGroup[],
  b: readonly ListGroup[],
): boolean {
  return (
    a.length === b.length &&
    a.every(
      (group, index) =>
        group.id === b[index].id &&
        group.label === b[index].label &&
        group.ids.length === b[index].ids.length &&
        group.ids.every((id, row) => id === b[index].ids[row]),
    )
  )
}

/** Every listed session, for the quick switcher, which searches them all. */
export const selectListedSessions = listed

/** The sessions waiting on the person, newest first, as ids. */
export const selectWaitingIds = createSelector([listed], (sessions) =>
  sessions
    .filter((session) => session.status === "needs-you")
    .sort(byRecency)
    .map((session) => session.id),
)
