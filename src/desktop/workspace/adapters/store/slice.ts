import type { ModelChoice } from "../../model/model-choice"
import type { FailureDiagnostic } from "../../model/failure"
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
import type { Channel, ModelRef, WorkspaceIndex } from "../../model/workspace-index"
import type {
  ChannelDraft,
  ChannelFailure,
  ChannelStep,
  ResourceKind,
} from "../../model/channel-draft"
import * as channelCreation from "../../application/usecases/channel-creation"
import type {
  Direction,
  PaneKey,
  Side,
  Zone,
} from "../../../split-panes/model/pane-layout"
import type { EdgeMove } from "../../../split-panes"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"
import type { Transcript, TurnMessage } from "../../model/transcript"
import type {
  Initiator,
  WorkspaceDependencies,
  WorkspaceUpdate,
} from "../../application/ports"
import type { StageMismatch, WorkspaceFailureReason } from "../../model/failure"
import type { WidgetRef } from "../../../widgets/model/widget-ref"
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
import * as access from "../../application/usecases/access"
import type { AccessMode } from "../../model/access"
import * as updates from "../../application/usecases/updates"
import * as sharing from "../../application/usecases/sharing"
import * as rowEditing from "../../application/usecases/row-editing"
import * as window from "../../application/usecases/window"
import type { WindowArrangement } from "../../model/shown-window"
import type { ShareDraft, ShareGrant, SharingChoices } from "../../model/sharing"
import type { ShareChange } from "../../application/workspace-state"

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
    closeInFront: (state, { payload }: Payload<{ draftId?: string }>) =>
      navigation.closeInFront(state, payload),
    widgetOpened: (
      state,
      {
        payload,
      }: Payload<{ widget: WidgetRef; origin?: string; room: PaneRoom | undefined }>,
    ) => panes.openWidget(state, payload),
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
    arrangementShown: (
      state,
      { payload }: Payload<{ arrangement: WindowArrangement | null }>,
    ) => window.arrangementShown(state, payload),
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
      {
        payload,
      }: Payload<{
        reason: WorkspaceFailureReason
        stages?: StageMismatch | null
        read: string
      }>,
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
      {
        payload,
      }: Payload<{ sessionId: string; message: TurnMessage; initiator: Initiator }>,
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
        diagnostic?: FailureDiagnostic
        model?: ModelRef
      }>,
    ) => sessions.sendFailed(state, payload),
    modelChoiceAsked: (
      state,
      { payload }: PayloadAction<{ sessionId: string; choice: ModelChoice }>,
    ) => {
      if (!Object.hasOwn(state.sessions, payload.sessionId)) return
      state.modelChoices[payload.sessionId] = payload.choice
    },
    modelChoiceSettled: (
      state,
      { payload }: PayloadAction<{ sessionId: string; choice: ModelChoice }>,
    ) => {
      const { sessionId, choice } = payload
      if (
        !Object.hasOwn(state.modelChoices, sessionId) ||
        state.modelChoices[sessionId].requestId !== choice.requestId
      )
        return
      state.modelChoices[sessionId] = choice
    },
    modelChoiceCleared: (
      state,
      { payload }: PayloadAction<{ sessionId: string; requestId: string }>,
    ) => {
      if (
        Object.hasOwn(state.modelChoices, payload.sessionId) &&
        state.modelChoices[payload.sessionId].requestId === payload.requestId
      )
        delete state.modelChoices[payload.sessionId]
    },
    chooseModel: (state, { payload }: Payload<{ sessionId: string; model: ModelRef }>) =>
      sessions.chooseModel(state, payload),
    stagedModelSettled: (
      state,
      { payload }: Payload<{ sessionId: string; model: ModelRef }>,
    ) => sessions.stagedModelSettled(state, payload),
    accessChosen: (
      state,
      { payload }: Payload<{ sessionId: string; mode: AccessMode }>,
    ) => access.accessChosen(state, payload),
    accessCaptured: (
      state,
      { payload }: Payload<{ sessionId: string; mode: AccessMode }>,
    ) => access.accessCaptured(state, payload),
    accessPending: (
      state,
      { payload }: Payload<{ sessionId: string; mode: AccessMode }>,
    ) => access.accessPending(state, payload),
    accessChanging: (
      state,
      { payload }: Payload<{ sessionId: string; mode: AccessMode }>,
    ) => access.accessChanging(state, payload),
    accessTaken: (state, { payload }: Payload<{ sessionId: string; mode: AccessMode }>) =>
      access.accessTaken(state, payload),
    accessRefused: (
      state,
      {
        payload,
      }: Payload<{
        sessionId: string
        mode: AccessMode
        reason: WorkspaceFailureReason
        code?: string
        requeue?: boolean
      }>,
    ) => access.accessRefused(state, payload),
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

    // A row's edits in place
    renameStarted: (state, { payload }: Payload<{ sessionId: string }>) =>
      rowEditing.renameStarted(state, payload),
    renameDrafted: (state, { payload }: Payload<{ sessionId: string; draft: string }>) =>
      rowEditing.renameDrafted(state, payload),
    renameEnded: (state, { payload }: Payload<{ sessionId: string }>) =>
      rowEditing.renameEnded(state, payload),
    deleteAsked: (state, { payload }: Payload<{ sessionId: string }>) =>
      rowEditing.deleteAsked(state, payload),
    deleteClosed: (state) => rowEditing.deleteClosed(state),

    // Sharing
    shareOpened: (
      state,
      { payload }: Payload<{ sessionId: string; opening: string; read: string }>,
    ) => sharing.shareOpened(state, payload),
    closeShare: (state) => sharing.shareClosed(state),
    sharesReading: (state, { payload }: Payload<{ opening: string; read: string }>) =>
      sharing.sharesReading(state, payload),
    sharesRead: (
      state,
      {
        payload,
      }: Payload<{
        opening: string
        read: string
        grants: sharing.ReadAnswer<readonly ShareGrant[]>
        choices: sharing.ReadAnswer<SharingChoices>
      }>,
    ) => sharing.sharesRead(state, payload),
    shareChanging: (
      state,
      { payload }: Payload<{ opening: string; change: ShareChange }>,
    ) => sharing.shareChanging(state, payload),
    shareChanged: (
      state,
      { payload }: Payload<{ settled: sharing.ChangeSettled; read: string }>,
    ) => sharing.shareChanged(state, payload),
    shareDrafted: (state, { payload }: Payload<{ opening: string; draft: ShareDraft }>) =>
      sharing.shareDrafted(state, payload),
    shareRestarted: (state, { payload }: Payload<{ opening: string }>) =>
      sharing.shareRestarted(state, payload),
    shareConfirming: (
      state,
      { payload }: Payload<{ opening: string; credentialId: string }>,
    ) => sharing.shareConfirming(state, payload),
    shareConfirmCancelled: (state, { payload }: Payload<{ opening: string }>) =>
      sharing.shareConfirmCancelled(state, payload),

    // Making a channel
    channelCreationOpened: (
      state,
      { payload }: Payload<{ opening: string; model: ModelRef }>,
    ) => channelCreation.channelCreationOpened(state, payload),
    channelCreationClosed: (state, { payload }: Payload<{ opening: string }>) =>
      channelCreation.channelCreationClosed(state, payload),
    channelDrafted: (
      state,
      {
        payload,
      }: Payload<{ opening: string; change: Partial<Omit<ChannelDraft, "resources">> }>,
    ) => channelCreation.channelDrafted(state, payload),
    channelResourcesAdded: (
      state,
      {
        payload,
      }: Payload<{ opening: string; paths: readonly string[]; kind: ResourceKind }>,
    ) => channelCreation.channelResourcesAdded(state, payload),
    channelResourceRemoved: (
      state,
      { payload }: Payload<{ opening: string; path: string }>,
    ) => channelCreation.channelResourceRemoved(state, payload),
    channelStepAsked: (
      state,
      { payload }: Payload<{ opening: string; step: ChannelStep }>,
    ) => channelCreation.channelStepAsked(state, payload),
    channelStepAnswered: (state, { payload }: Payload<{ opening: string }>) =>
      channelCreation.channelStepAnswered(state, payload),
    channelCreating: (
      state,
      { payload }: Payload<{ opening: string; requestId: string }>,
    ) => channelCreation.channelCreating(state, payload),
    channelCreationFailed: (
      state,
      { payload }: Payload<{ opening: string; failure: ChannelFailure }>,
    ) => channelCreation.channelCreationFailed(state, payload),
    channelHomeKnown: (state, { payload }: Payload<{ home: string }>) => ({
      ...state,
      channelMaking: { ...state.channelMaking, home: payload.home },
    }),
    channelMade: (
      state,
      { payload }: Payload<{ opening: string; channel: Channel; draftId: string }>,
    ) => channelCreation.channelMade(state, payload),
  },
})

export const workspaceActions = workspaceSlice.actions

/**
 * The workspace's first state, with what was kept between launches — the
 * overview's filter — whether its sessions can be shared from here, and
 * whether channels can be made here and their paths picked.
 */
export function initialWorkspaceFrom({
  overviewFilter,
  sharing,
  channels,
  paths,
}: Pick<
  WorkspaceDependencies,
  "overviewFilter" | "sharing" | "channels" | "paths"
>): WorkspaceState {
  return {
    ...initialWorkspace,
    overview: { ...initialWorkspace.overview, filter: overviewFilter.read() },
    shareable: sharing !== undefined,
    channelMaking: {
      creatable: channels !== undefined,
      pickable: paths !== undefined,
      home: null,
    },
  }
}

/**
 * The actions that go somewhere — a session, a channel, a new
 * session, a widget's pane, another pane (or the focused one, asked for by
 * name). Each leaves the Agents overview, or a widget over the panes, for
 * the panes (`navigated`), whoever dispatched it —
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
    workspaceActions.widgetOpened,
    workspaceActions.selectChannel,
    workspaceActions.channelOpened,
  ].map((action) => action.type),
)

/**
 * The actions that change the panes a person asked to change: closing,
 * moving, evening them out, resizing. Each leaves the overview, or a widget
 * over the panes, only when it changed them, so no pane changes unseen
 * beneath it; one that changes nothing — a move at the edge, a close of no
 * pane — leaves the view as it is. Fitting the panes to the window is not
 * asked by anyone, and leaves the view as it is.
 */
const changesPanes: ReadonlySet<string> = new Set(
  [
    workspaceActions.paneMoved,
    workspaceActions.paneNudged,
    workspaceActions.paneClosed,
    workspaceActions.closeInFront,
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
