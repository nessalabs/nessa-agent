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
import type { Direction, PaneKey, Side, Zone } from "../../model/pane-layout"
import type { PaneEdge, WorkspaceRoom } from "../../model/pane-sizing"
import type { AttentionStatus } from "../../model/session-groups"
import type { Message, Transcript } from "../../model/transcript"
import type { WorkspaceUpdate } from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import { initialWorkspace, type WorkspaceState } from "../../application/workspace-state"
import * as navigation from "../../application/usecases/navigation"
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
        room: WorkspaceRoom | undefined
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
        room: WorkspaceRoom | undefined
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
        room: WorkspaceRoom | undefined
      }>,
    ) => panes.movePane(state, payload),
    paneNudged: (
      state,
      {
        payload,
      }: Payload<{
        pane: PaneKey
        direction: Direction
        room: WorkspaceRoom | undefined
      }>,
    ) => panes.nudgePane(state, payload),
    panesFitted: (
      state,
      { payload }: Payload<{ room: { width: number; height: number } }>,
    ) => panes.fitPanes(state, payload),
    resizePanes: (
      state,
      { payload }: Payload<{ edge: PaneEdge; fraction: number; pair: number }>,
    ) => panes.resizePanes(state, payload),
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
        room?: WorkspaceRoom
      }>,
    ) => panes.createDraft(state, payload),

    // Navigation
    selectChannel: (state, { payload }: Payload<{ channelId: string }>) =>
      navigation.selectChannel(state, payload),
    selectStatusView: (state, { payload }: Payload<{ status: AttentionStatus }>) =>
      navigation.selectStatusView(state, payload),
    channelOpened: (
      state,
      {
        payload,
      }: Payload<{
        channelId: string
        beside?: boolean
        draftId?: string
        room?: WorkspaceRoom
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
    indexLoaded: (
      state,
      { payload }: Payload<{ index: WorkspaceIndex; draftId: string }>,
    ) => updates.indexLoaded(state, payload),
    indexRequested: (state) => updates.indexRequested(state),
    indexFailed: (state, { payload }: Payload<{ reason: WorkspaceFailureReason }>) =>
      updates.indexFailed(state, payload),
    updateReceived: (
      state,
      {
        payload,
      }: Payload<{ update: Exclude<WorkspaceUpdate, { kind: "session-removed" }> }>,
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
    messageSent: (state, { payload }: Payload<{ sessionId: string; message: Message }>) =>
      sessions.messageSent(state, payload),
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
export const workspaceReducer = workspaceSlice.reducer
