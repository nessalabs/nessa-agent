/**
 * The workspace in the desktop store. Each case applies one use case from
 * `application/usecases/`, so the rules live there and this file only names
 * the actions. Commands a person or an agent issues are named for what they
 * ask (`openBeside`, `closePane`); what the workspace reports to itself is
 * named for what happened (`messageSent`, `updateReceived`). Every action is
 * plain data. Commands that need an id, the clock or the source are thunks in
 * `commands.ts`, which dispatch these.
 */
import { createSlice, type PayloadAction } from "@reduxjs/toolkit"
import type { ModelRef, WorkspaceIndex } from "../../model/workspace-index"
import type {
  Direction,
  PaneKey,
  Side,
  Zone,
} from "../../../split-panes/model/pane-layout"
import type { EdgeMove } from "../../../split-panes"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"
import type { Message, Transcript } from "../../model/transcript"
import type {
  Initiator,
  WorkspaceDependencies,
  WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import {
  initialWorkspace,
  type ContentView,
  type WorkspaceState,
} from "../../application/workspace-state"
import * as navigation from "../../application/usecases/navigation"
import * as overview from "../../application/usecases/overview"
import type { AgentsFilter } from "../../model/overview/filter"
import type { AgentsGroup } from "../../model/overview/agents-glance"
import * as panes from "../../application/usecases/panes"
import * as sessions from "../../application/usecases/sessions"
import * as updates from "../../application/usecases/updates"

type Payload<T> = PayloadAction<T>

const workspaceSlice = createSlice({
  name: "workspace",
  initialState: initialWorkspace as WorkspaceState,
  reducers: {
    // Panes
    focusPane: (state, { payload }: Payload<{ pane: PaneKey }>) =>
      panes.focusPane(state, payload.pane),
    openSession: (state, { payload }: Payload<{ sessionId: string; pane?: PaneKey }>) =>
      panes.openSession(state, payload),
    openedBeside: (
      state,
      {
        payload,
      }: Payload<{
        sessionId: string
        target?: PaneKey
        side?: Side
        room: PaneRoom | undefined
        replace?: boolean
      }>,
    ) => panes.openBeside(state, payload),
    sessionDropped: (
      state,
      {
        payload,
      }: Payload<{
        sessionId: string
        target: PaneKey
        zone: Zone
        room: PaneRoom | undefined
      }>,
    ) => panes.dropSession(state, payload),
    paneMoved: (
      state,
      {
        payload,
      }: Payload<{
        pane: PaneKey
        target: PaneKey
        zone: Zone
        room: PaneRoom | undefined
      }>,
    ) => panes.movePane(state, payload),
    paneNudged: (
      state,
      {
        payload,
      }: Payload<{
        pane: PaneKey
        direction: Direction
        room: PaneRoom | undefined
      }>,
    ) => panes.nudgePane(state, payload),
    panesFitted: (
      state,
      { payload }: Payload<{ room: { width: number; height: number } }>,
    ) => panes.fitPanes(state, payload),
    resizePanes: (state, { payload }: Payload<EdgeMove>) =>
      panes.resizePanes(state, payload),
    equalizePanes: (state) => panes.equalizePanes(state),
    paneClosed: (state, { payload }: Payload<{ pane: PaneKey; draftId?: string }>) =>
      panes.closePane(state, payload),
    draftCreated: (
      state,
      {
        payload,
      }: Payload<{
        draftId: string
        channelId?: string
        model?: ModelRef
        beside?: Side
        target?: PaneKey
        room?: PaneRoom
      }>,
    ) => panes.createDraft(state, payload),

    // Navigation
    showContent: (state, { payload }: Payload<{ content: ContentView }>) =>
      navigation.showContent(state, payload),
    selectInOverview: (state, { payload }: Payload<{ sessionId: string }>) =>
      overview.selectInOverview(state, payload),
    overviewChoiceKept: (state, { payload }: Payload<{ now: number }>) =>
      overview.keepOverviewChoice(state, payload),
    filterOverview: (state, { payload }: Payload<{ filter: AgentsFilter }>) =>
      overview.filterOverview(state, payload),
    showOverviewGroup: (state, { payload }: Payload<{ group: AgentsGroup | null }>) =>
      overview.showOverviewGroup(state, payload),
    selectChannel: (state, { payload }: Payload<{ channelId: string }>) =>
      navigation.selectChannel(state, payload),
    channelOpened: (
      state,
      {
        payload,
      }: Payload<{
        channelId: string
        beside?: boolean
        draftId?: string
        room?: PaneRoom
      }>,
    ) => navigation.openChannel(state, payload),
    revealSession: (state, { payload }: Payload<{ sessionId: string }>) =>
      navigation.revealSession(state, payload),
    toggleSection: (state, { payload }: Payload<{ sectionId: string }>) =>
      navigation.toggleSection(state, payload),
    toggleChannel: (state, { payload }: Payload<{ channelId: string; open?: boolean }>) =>
      navigation.toggleChannel(state, payload),
    toggleShowAll: (state, { payload }: Payload<{ channelId: string }>) =>
      navigation.toggleShowAll(state, payload),
    toggleSidebar: (state, { payload }: Payload<{ open?: boolean } | undefined>) =>
      navigation.toggleSidebar(state, payload ?? {}),
    toggleSessionList: (state, { payload }: Payload<{ open?: boolean } | undefined>) =>
      navigation.toggleSessionList(state, payload ?? {}),
    resizeSidebar: (state, { payload }: Payload<{ width: number }>) =>
      navigation.resizeSidebar(state, payload),
    resizeSessionList: (state, { payload }: Payload<{ width: number }>) =>
      navigation.resizeSessionList(state, payload),
    fitToWindow: (
      state,
      {
        payload,
      }: Payload<{ windowWidth: number; sidebarWidth: number; sessionListWidth: number }>,
    ) => navigation.fitToWindow(state, payload),

    // Sessions
    // Each read of the index is named (`read`), so what follows its answer —
    // the resync's cutoff (`effects.ts`) — is paired with the read that asked,
    // whatever order the answers arrive in.
    indexLoaded: (
      state,
      { payload }: Payload<{ index: WorkspaceIndex; draftId: string; read: string }>,
    ) => updates.indexLoaded(state, payload),
    indexRequested: (state, { payload }: Payload<{ read: string }>) =>
      updates.indexRequested(state, payload),
    indexFailed: (
      state,
      { payload }: Payload<{ reason: WorkspaceFailureReason; read: string }>,
    ) => updates.indexFailed(state, payload),
    updateReceived: (
      state,
      {
        payload,
      }: Payload<{
        update: Extract<WorkspaceUpdate, { kind: "session" | "transcript" }>
      }>,
    ) => updates.updateReceived(state, payload),
    sessionRemoved: (
      state,
      { payload }: Payload<{ sessionId: string; revision: number; draftId: string }>,
    ) => updates.sessionRemoved(state, payload),
    transcriptLoaded: (state, { payload }: Payload<{ transcript: Transcript }>) =>
      updates.transcriptLoaded(state, payload),
    transcriptFailed: (
      state,
      { payload }: Payload<{ sessionId: string; reason: WorkspaceFailureReason }>,
    ) => updates.transcriptFailed(state, payload),
    transcriptRetried: (state, { payload }: Payload<{ sessionId: string }>) =>
      updates.transcriptRetried(state, payload),
    messageSent: (
      state,
      { payload }: Payload<{ sessionId: string; message: Message; initiator: Initiator }>,
    ) => sessions.messageSent(state, payload),
    messageDelivered: (
      state,
      { payload }: Payload<{ sessionId: string; messageId: string }>,
    ) => sessions.messageDelivered(state, payload),
    sendFailed: (
      state,
      {
        payload,
      }: Payload<{
        sessionId: string
        messageId: string
        reason: WorkspaceFailureReason
      }>,
    ) => sessions.sendFailed(state, payload),
    chooseModel: (state, { payload }: Payload<{ sessionId: string; model: ModelRef }>) =>
      sessions.chooseModel(state, payload),
    approvalAnswering: (
      state,
      { payload }: Payload<{ sessionId: string; approvalId: string; token: string }>,
    ) => sessions.approvalAnswering(state, payload),
    approvalFailed: (
      state,
      {
        payload,
      }: Payload<{
        sessionId: string
        approvalId: string
        token: string
        reason: WorkspaceFailureReason
      }>,
    ) => sessions.approvalFailed(state, payload),
    messageResent: (
      state,
      { payload }: Payload<{ sessionId: string; messageId: string }>,
    ) => sessions.messageResent(state, payload),
    unsentDiscarded: (
      state,
      { payload }: Payload<{ sessionId: string; messageId: string }>,
    ) => sessions.unsentDiscarded(state, payload),
    sessionRead: (state, { payload }: Payload<{ sessionId: string }>) =>
      sessions.sessionRead(state, payload),
    setComposerText: (state, { payload }: Payload<{ sessionId: string; text: string }>) =>
      sessions.composerTextChanged(state, payload),
  },
})

export const workspaceActions = workspaceSlice.actions

/** The workspace's first state, with what was kept between launches: the overview's filter. */
export function initialWorkspaceFrom({
  overviewFilter,
}: Pick<WorkspaceDependencies, "overviewFilter">): WorkspaceState {
  return {
    ...initialWorkspace,
    overview: { ...initialWorkspace.overview, filter: overviewFilter.read() },
  }
}

/**
 * The actions that go somewhere — a session, a channel, a new
 * session, another pane (or the focused one, asked for by name). Each leaves
 * the Agents overview for the panes (`navigated`), whoever dispatched it —
 * the sidebar, the switcher, a key, the overview's own Open, or an agent —
 * even when it finds the window already there: going to a place is going
 * there.
 */
const goesSomewhere: ReadonlySet<string> = new Set(
  [
    workspaceActions.focusPane,
    workspaceActions.openSession,
    workspaceActions.openedBeside,
    workspaceActions.sessionDropped,
    workspaceActions.draftCreated,
    workspaceActions.selectChannel,
    workspaceActions.channelOpened,
  ].map((action) => action.type),
)

/**
 * The actions that change the panes a person asked to change: closing,
 * moving, evening them out, resizing. Each leaves the overview only when it
 * changed them, so no pane changes unseen beneath it; one that changes
 * nothing — a move at the edge, a close of no pane — leaves the view as it
 * is. Fitting the panes to the window is not asked by anyone, and leaves the
 * view as it is.
 */
const changesPanes: ReadonlySet<string> = new Set(
  [
    workspaceActions.paneMoved,
    workspaceActions.paneNudged,
    workspaceActions.paneClosed,
    workspaceActions.equalizePanes,
    workspaceActions.resizePanes,
  ].map((action) => action.type),
)

export const workspaceReducer: typeof workspaceSlice.reducer = (state, action) => {
  const next = workspaceSlice.reducer(state, action)
  if (goesSomewhere.has(action.type)) return navigation.navigated(next)
  if (changesPanes.has(action.type) && next.panes !== state?.panes)
    return navigation.navigated(next)
  return next
}
