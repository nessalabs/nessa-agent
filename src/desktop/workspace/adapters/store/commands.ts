/**
 * The workspace's commands: what a person or an agent dispatches to drive the
 * window. This module is the one list. The plain ones are the slice's
 * actions, re-exported here. Those that need a fresh id, the time, or the
 * source are thunks in the file that owns them — the index, the panes, or
 * the session — and re-exported here, taking what they need from the store's
 * extra argument.
 *
 * ```ts
 * store.dispatch(openBeside({ sessionId: "retry" }))
 * store.dispatch(closePane())              // the focused pane
 * store.dispatch(openWidget({ widget: { plugin: "experiments", id: "run" }, place: "window" }))
 * await store.dispatch(sendMessage({ sessionId: "retry", text: "Run it.", initiator: "agent" }))
 * ```
 */
import type { ThunkAction, UnknownAction } from "@reduxjs/toolkit"
import type { WorkspaceDependencies } from "../../application/ports"
import type { WorkspaceState } from "../../application/workspace-state"
import { workspaceActions } from "./slice"

export const {
  focusPane,
  openSession,
  resizePanes,
  equalizePanes,
  selectChannel,
  showContent,
  selectInOverview,
  filterOverview,
  showOverviewGroup,
  revealSession,
  toggleSection,
  toggleChannel,
  toggleShowAll,
  toggleSidebar,
  toggleSessionList,
  searchList,
  resizeSidebar,
  resizeSessionList,
  fitToWindow,
  chooseModel,
  setComposerText,
  transcriptRetried: retryTranscript,
  unsentDiscarded: discardUnsent,
  closeShare,
  renameStarted,
  renameDrafted,
  renameEnded,
  deleteAsked,
  deleteClosed,
  arrangementShown,
  panelShown,
  panelTabShown: showPanelTab,
  panelTabClosed: closePanelTab,
} = workspaceActions

export type WorkspaceCommand<Result = void> = ThunkAction<
  Result,
  { workspace: WorkspaceState },
  WorkspaceDependencies,
  UnknownAction
>

export { followWorkspace, loadWorkspace } from "./index-commands"
export {
  canNudge,
  canOpenBeside,
  closeInFront,
  closePane,
  commitDrop,
  dropSession,
  fitPanes,
  measureRoom,
  movePane,
  newSession,
  nudgePane,
  openBeside,
  openChannel,
  openWidget,
} from "./pane-commands"
export {
  DRAFT_IMAGE_LIMITS,
  addComposerImages,
  answerQuestion,
  approve,
  archiveSession,
  deleteSession,
  deny,
  pinSession,
  providerLoginAvailable,
  removeComposerImage,
  renameSession,
  forkSession,
  loadEarlier,
  resendMessage,
  retryReview,
  sendMessage,
  signInToProvider,
  stopTurn,
  type AnswerOutcome,
  type ChangeOutcome,
  type QuestionOutcome,
  type SendOutcome,
} from "./session-commands"
export {
  askRevoke,
  cancelRevoke,
  draftShare,
  openShare,
  readShares,
  restartShare,
  revokeShare,
  shareSession,
} from "./sharing-commands"
export { chooseAccess, type AccessOutcome } from "./access-commands"
export { setUsageLimitAutoContinue } from "./usage-limit-commands"
export { changeModel } from "./model-commands"
export {
  addChannelResources,
  answerChannelStep,
  askChannelStep,
  chooseChannelFolder,
  closeChannelCreation,
  createChannel,
  draftChannel,
  openChannelCreation,
  removeChannelResource,
  setChannelPlacement,
  watchChannelDrops,
} from "./channel-commands"
