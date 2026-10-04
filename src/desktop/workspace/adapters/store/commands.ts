/**
 * The workspace's commands: what a person or an agent dispatches to drive the
 * window. The plain ones are the slice's actions, re-exported so this module
 * is the one list; those that need a fresh id, the time or the source are
 * thunks here, taking them from the store's extra argument.
 *
 * ```ts
 * store.dispatch(openBeside({ sessionId: "retry" }))
 * store.dispatch(closePane())              // the focused pane
 * store.dispatch(openWidget({ widget: { plugin: "experiments", id: "run" }, place: "window" }))
 * await store.dispatch(sendMessage({ sessionId: "retry", text: "Run it.", initiator: "agent" }))
 * ```
 */
import type { ThunkAction, UnknownAction } from "@reduxjs/toolkit"
import {
  type Direction,
  type PaneKey,
  type Side,
  type Zone,
} from "../../../split-panes/model/pane-layout"
import { messageText, type Message } from "../../model/transcript"
import {
  failureReason,
  type ApprovalScope,
  type Initiator,
  type WorkspaceDependencies,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import { consistentIndex, contradicts, type ModelRef } from "../../model/workspace-index"
import { paneItemOf } from "../../model/pane-item"
import type { WidgetRef } from "../../../widgets/model/widget-ref"
import { fromSource, knownToSource } from "../../model/revision"
import {
  answering,
  draftOf,
  entry,
  modelForNextTurn,
  onScreen,
  sessionOf,
  type WorkspaceState,
} from "../../application/workspace-state"
import * as panesUseCases from "../../application/usecases/panes"
import type { Drop } from "../../../split-panes"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"
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
  resizeSidebar,
  resizeSessionList,
  fitToWindow,
  chooseModel,
  setComposerText,
  transcriptRetried: retryTranscript,
  unsentDiscarded: discardUnsent,
} = workspaceActions

const {
  openedBeside,
  sessionDropped,
  paneMoved,
  paneNudged,
  panesFitted,
  indexLoaded,
  indexFailed,
  updateReceived,
  sessionRemoved,
  draftCreated,
  paneClosed,
  closeInFront: closedInFront,
  widgetOpened,
  channelOpened,
  messageSent,
  messageDelivered,
  messageResent,
  sendFailed,
  approvalAnswering,
  approvalFailed,
  indexRequested,
} = workspaceActions

export type WorkspaceCommand<Result = void> = ThunkAction<
  Result,
  { workspace: WorkspaceState },
  WorkspaceDependencies,
  UnknownAction
>

/**
 * Reads the index and opens the workspace on it. Open already, it is the
 * resync for whatever the stream lost: a session the index no longer lists
 * is taken out, and the conversations the panes show are read again.
 */
export function loadWorkspace(): WorkspaceCommand<Promise<void>> {
  return async (dispatch, _getState, { workspace, newId }) => {
    const read = newId()
    dispatch(indexRequested({ read }))
    try {
      const index = await workspace.index()
      const unusable = index.sessions.filter((session) => !fromSource(session))
      // Let go by the reducer; said here, where the source's answer is still at hand.
      if (unusable.length > 0)
        console.warn(
          "The index listed sessions at a revision the source could not have sent",
          unusable.map((session) => session.id),
        )
      // Left out by the reducer (`consistentIndex`); said here, by id.
      const { contradictions } = consistentIndex(index)
      if (contradicts(contradictions))
        console.warn("The index contradicted itself; left out, by id:", contradictions)
      dispatch(indexLoaded({ index, draftId: newId(), read }))
    } catch (error) {
      dispatch(indexFailed({ reason: failureReason(error), read }))
    }
  }
}

/** The session an update is of, whichever it is. */
function sessionOfUpdate(update: Exclude<WorkspaceUpdate, { kind: "resync" }>): string {
  switch (update.kind) {
    case "session":
      return update.session.id
    case "session-removed":
      return update.sessionId
    case "transcript":
      return update.transcript.sessionId
  }
}

/** The revision an update carries, whichever it is. */
function revisionOf(update: Exclude<WorkspaceUpdate, { kind: "resync" }>): number {
  switch (update.kind) {
    case "session":
      return update.session.revision
    case "session-removed":
      return update.revision
    case "transcript":
      return update.transcript.revision
  }
}

/**
 * Follows the source's changes until the returned function is called. An
 * update at a revision the source could not have sent is let go, and said.
 * A `resync` — the source may have lost updates — reads the index again
 * (`loadWorkspace`), and with it every conversation on screen.
 */
export function followWorkspace(): WorkspaceCommand<() => void> {
  return (dispatch, _getState, { workspace, newId }) =>
    workspace.subscribe((update) => {
      if (update.kind === "resync") {
        void dispatch(loadWorkspace())
        return
      }
      if (!fromSource({ revision: revisionOf(update) })) {
        // Said by what it is and whose, never with what it holds: a transcript
        // carries the person's words, code and commands.
        console.warn("The workspace source sent an update it could not have sent", {
          kind: update.kind,
          sessionId: sessionOfUpdate(update),
          revision: revisionOf(update),
        })
        return
      }
      dispatch(
        update.kind === "session-removed"
          ? // A removal may empty the last pane, which then starts over.
            sessionRemoved({
              sessionId: update.sessionId,
              revision: update.revision,
              draftId: newId(),
            })
          : updateReceived({ update }),
      )
    })
}

/**
 * Opens a session beside a pane — the focused one unless `target` is named —
 * on `side`, or, with none named, to the right, else below; only where the
 * room the page measures allows (`arrange`). Where it will not fit, the
 * session takes the target's place, unless `replace` is false. A session on
 * screen already is focused where it is.
 */
export function openBeside(options: {
  sessionId: string
  target?: PaneKey
  side?: Side
  replace?: boolean
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(openedBeside({ ...options, room: measure() }))
  }
}

/**
 * Opens a session on a pane's zone, as a drop there would: the middle opens
 * it there, a side beside it there where the room the page measures now
 * allows, else nothing. An agent's way to do what a drag does.
 */
export function dropSession(options: {
  sessionId: string
  target: PaneKey
  zone: Zone
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(sessionDropped({ ...options, room: measure() }))
  }
}

/**
 * Moves a pane to a zone of another: the middle swaps them; a side only where
 * the room the page measures now allows. An agent's way to do what a drag does.
 */
export function movePane(options: {
  pane: PaneKey
  target: PaneKey
  zone: Zone
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(paneMoved({ ...options, room: measure() }))
  }
}

/**
 * The keyboard's move: a swap with the pane that way, or a column of its own
 * where the room allows.
 */
export function nudgePane(options: {
  pane: PaneKey
  direction: Direction
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(paneNudged({ ...options, room: measure() }))
  }
}

/**
 * Holds every pane to the readable size in the room the page measures now —
 * after the window or a side column changed it — rebalancing where it must.
 */
export function fitPanes(): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    const room = measure()
    if (room) dispatch(panesFitted({ room }))
  }
}

/**
 * The panes' room as the page measures it now: what the split panes' drag
 * reads once, as its press begins (`split-panes-source.ts`).
 */
export function measureRoom(): WorkspaceCommand<PaneRoom | undefined> {
  return (_dispatch, _getState, { measure }) => measure()
}

/**
 * The drag's drop: commits what the drag previewed for the same zone
 * (`dropOutcome` of the same panes), in the same room — the one read as the
 * press began, so the drop reads nothing of the page. The drag's own,
 * through the split panes' source; an agent moves a pane with `movePane` or
 * opens a session with `dropSession`, which measure the room as they run.
 */
export function commitDrop({ carried, target, zone, room }: Drop): WorkspaceCommand {
  return (dispatch) => {
    if (carried.kind === "pane") {
      dispatch(paneMoved({ pane: carried.pane, target, zone, room }))
      return
    }
    // Carried from outside the grid: a session's row. Nothing carries a widget in.
    const item = paneItemOf(carried.item)
    if (item?.kind === "session")
      dispatch(sessionDropped({ sessionId: item.sessionId, target, zone, room }))
  }
}

/**
 * Whether a session could open beside `target` (the focused pane) on `side`,
 * or on either when none is named, without taking its place. What a menu or
 * the switcher asks before it offers "beside", so it never promises what the
 * command will not do.
 */
export function canOpenBeside(
  options: { target?: PaneKey; side?: Side } = {},
): WorkspaceCommand<boolean> {
  return (_dispatch, getState, { measure }) =>
    panesUseCases.canOpenBeside(getState().workspace, { ...options, room: measure() })
}

/** Whether a pane can be moved that way: what the Move items ask before they offer it. */
export function canNudge(options: {
  pane: PaneKey
  direction: Direction
}): WorkspaceCommand<boolean> {
  return (_dispatch, getState, { measure }) =>
    panesUseCases.canNudge(getState().workspace, { ...options, room: measure() })
}

/**
 * Starts a new session as a draft: in the focused pane (or `target`), or
 * beside it on `beside` where the room allows, in the channel being looked
 * at unless one is named. Returns the draft's id, or nothing when no pane
 * took it.
 */
export function newSession(
  options: {
    beside?: Side
    target?: PaneKey
    channelId?: string
  } = {},
): WorkspaceCommand<string | undefined> {
  return (dispatch, getState, { newId, measure }) => {
    const draftId = newId()
    dispatch(
      draftCreated({ draftId, ...options, room: options.beside ? measure() : undefined }),
    )
    return draftOf(getState().workspace, draftId) ? draftId : undefined
  }
}

/**
 * Closes a pane — the focused one unless `pane` is named. The last pane goes
 * back to a new session's home instead.
 */
export function closePane({ pane }: { pane?: PaneKey } = {}): WorkspaceCommand {
  return (dispatch, getState, { newId }) => {
    const panes = getState().workspace.panes
    if (!panes) return
    dispatch(paneClosed({ pane: pane ?? panes.focused, draftId: newId() }))
  }
}

/**
 * Closes what is in front (⌘W): a widget over the panes, back to the panes,
 * never a pane beneath it; else the focused pane, which, the last, goes
 * back to a new session's home.
 */
export function closeInFront(): WorkspaceCommand {
  return (dispatch, _getState, { newId }) => {
    dispatch(closedInFront({ draftId: newId() }))
  }
}

/**
 * Opens a widget (ADR 326): in a pane of its own — beside the pane showing
 * `origin`, the session the caller says it belongs to, where the room the
 * page measures allows, else in the focused pane's place — or over the
 * panes, in the window, replacing any widget there.
 */
export function openWidget({
  widget,
  place,
  origin,
}: {
  widget: WidgetRef
  place: "pane" | "window"
  origin?: string
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(
      place === "window"
        ? showContent({ content: { widget } })
        : widgetOpened({ widget, origin, room: measure() }),
    )
  }
}

/**
 * Opens a channel's newest session, in place or beside, or a new session
 * when it has none.
 */
export function openChannel({
  channelId,
  beside = false,
}: {
  channelId: string
  beside?: boolean
}): WorkspaceCommand {
  return (dispatch, _getState, { newId, measure }) => {
    dispatch(
      channelOpened({
        channelId,
        beside,
        draftId: newId(),
        room: beside ? measure() : undefined,
      }),
    )
  }
}

/**
 * Asks the source to take a message already in the outbox. While the source
 * has not spoken of the session — it began here, at revision 0 — the message
 * carries what the session starts with, which the source takes once however
 * often it is sent. A refusal marks the message as not sent, with the reason.
 */
function deliver(
  sessionId: string,
  message: Message,
  model: ModelRef,
  initiator: Initiator,
): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session) return "not-asked"
    try {
      await workspace.send({
        sessionId,
        initiator,
        messageId: message.id,
        text: messageText(message),
        model,
        start: knownToSource(session)
          ? undefined
          : { channelId: session.channelId, title: session.title },
      })
      dispatch(messageDelivered({ sessionId, messageId: message.id }))
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      dispatch(sendFailed({ sessionId, messageId: message.id, reason }))
      return outcomeOf(reason)
    }
  }
}

/**
 * What became of a message, so a caller can tell what it did: `sent` and
 * taken, `refused` or `unknown` (`outcomeOf`) — the message stays in the
 * outbox, marked not sent — or `not-asked`: nothing to send, or no session
 * or draft to send it to.
 */
export type SendOutcome = "sent" | "refused" | "unknown" | "not-asked"

/**
 * Sends a message. It shows at once in the outbox, marked as sending; a draft
 * starts with it. The source's own updates then carry the reply; a refused
 * message stays, to be sent again or let go.
 */
export function sendMessage({
  sessionId,
  text,
  initiator,
}: {
  sessionId: string
  text: string
  /** Who sent it, for the source's record: the person at the composer, or an agent. */
  initiator: Initiator
}): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState, { now, newId }) => {
    const content = text.trim()
    const model = modelForNextTurn(getState().workspace, sessionId)
    if (!content || !model) return "not-asked"
    const message: Message = {
      id: newId(),
      role: "user",
      at: now(),
      parts: [{ kind: "text", text: content }],
      delivery: { state: "sending" },
    }
    dispatch(messageSent({ sessionId, message, initiator }))
    return dispatch(deliver(sessionId, message, model, initiator))
  }
}

/**
 * Sends a refused message again, in its place and under its id, so a source
 * that did take it the first time takes it once.
 */
export function resendMessage({
  sessionId,
  messageId,
  initiator,
}: {
  sessionId: string
  messageId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState) => {
    const state = getState().workspace
    const refused = entry(state.outbox, sessionId)?.find(
      (message) => message.id === messageId && message.delivery?.state === "failed",
    )
    const model = modelForNextTurn(state, sessionId)
    if (!refused || !model) return "not-asked"
    dispatch(messageResent({ sessionId, messageId }))
    return dispatch(deliver(sessionId, refused, model, initiator))
  }
}

/**
 * What became of an answer to an approval, so a caller — an agent above all —
 * can tell what it did: `sent` and taken; `refused` by the source (the pane
 * says why while it shows the session); `unknown` when it is not known to be
 * taken (`outcomeOf`); `answering` already, a first answer on
 * its way; or `not-asked` — the window holds no such approval: the session is
 * not on screen (`onScreen`: in a pane, or in the open overview), or its
 * conversation has moved on. Nothing is sent
 * then, and the source records nothing, since nothing was asked of it.
 */
export type AnswerOutcome = "sent" | "refused" | "unknown" | "answering" | "not-asked"

/**
 * A call that failed, as its caller hears it: `unknown` for `unavailable`
 * (what that covers is `failure.ts`'s to say), `refused` for every other
 * reason. The source's updates say what became of it.
 */
function outcomeOf(reason: WorkspaceFailureReason): "refused" | "unknown" {
  return reason === "unavailable" ? "unknown" : "refused"
}

/**
 * Answers the approval named — the one seen. An approval the conversation no
 * longer asks, replaced meanwhile by another, is not answered: nobody has
 * seen the new one.
 */
function answer(
  sessionId: string,
  approvalId: string,
  send: (dependencies: WorkspaceDependencies) => Promise<void>,
): WorkspaceCommand<Promise<AnswerOutcome>> {
  return async (dispatch, getState, dependencies) => {
    const state = getState().workspace
    const approval = entry(state.transcripts, sessionId)?.approval
    // Only an approval on screen is answered: one a pane or the open overview shows.
    if (!onScreen(state, sessionId) || approval?.id !== approvalId) return "not-asked"
    // One answer at a time: a second waits until the first is refused.
    if (answering(entry(state.answers, sessionId), approvalId)) return "answering"
    const token = dependencies.newId()
    dispatch(approvalAnswering({ sessionId, approvalId: approval.id, token }))
    try {
      await send(dependencies)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      dispatch(approvalFailed({ sessionId, approvalId: approval.id, token, reason }))
      return outcomeOf(reason)
    }
  }
}

/**
 * Allows what the session's agent is waiting to run: this once, or always.
 * Who decided goes with it — the person at the window's controls, or an agent
 * — for the source's record of the decision.
 */
export function approve({
  sessionId,
  approvalId,
  scope = "once",
  initiator,
}: {
  sessionId: string
  approvalId: string
  scope?: ApprovalScope
  initiator: Initiator
}): WorkspaceCommand<Promise<AnswerOutcome>> {
  return answer(sessionId, approvalId, ({ workspace }) =>
    workspace.approve(sessionId, approvalId, scope, initiator),
  )
}

/** Refuses what the session's agent is waiting to run. */
export function deny({
  sessionId,
  approvalId,
  initiator,
}: {
  sessionId: string
  approvalId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<AnswerOutcome>> {
  return answer(sessionId, approvalId, ({ workspace }) =>
    workspace.deny(sessionId, approvalId, initiator),
  )
}

/**
 * What became of a pin or an archive asked of the source: `sent` and taken;
 * `refused` (logged); `unknown` when it is not known to be taken (`outcomeOf`); or
 * `not-asked` — nothing to change, or a session the source has not spoken
 * of. Each call answers for itself: of two archives of
 * one session asked at once, the second is `refused` as `unknown-session`
 * once the first has taken it; two pins asked at once are both `sent`, and
 * both on the source's record.
 */
export type ChangeOutcome = "sent" | "refused" | "unknown" | "not-asked"

/**
 * Pins a session under its channel in the sidebar, or unpins it, saying who
 * asked for the source's record. The pin shows when the source's update says
 * so; a refusal leaves it as it was.
 */
export function pinSession({
  sessionId,
  pinned,
  initiator,
}: {
  sessionId: string
  pinned: boolean
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || session.pinned === pinned || !knownToSource(session))
      return "not-asked"
    try {
      await workspace.setPinned(sessionId, pinned, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not pin a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Archives a session, saying who asked for the source's record. The source
 * reports the removal, and that update takes it out of the lists and closes
 * its pane, or starts the last over. A session the source has not spoken of
 * is not archived: it is let go by discarding its unsent message. A refusal
 * leaves the session where it is.
 */
export function archiveSession({
  sessionId,
  initiator,
}: {
  sessionId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || !knownToSource(session)) return "not-asked"
    try {
      await workspace.archive(sessionId, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not archive a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/** Opens a provider's login, leaving the failed turn and its recovery notice intact. */
export const signInToProvider =
  (provider: "claude" | "codex"): WorkspaceCommand<Promise<void>> =>
  async (_dispatch, _getState, dependencies) => {
    if (!dependencies.signInToProvider)
      throw new Error("Provider sign-in is unavailable on this host.")
    await dependencies.signInToProvider(provider)
  }

/** Read the injected host's login capability without attempting a launch. */
export const providerLoginAvailable =
  (): WorkspaceCommand<Promise<boolean>> => async (_dispatch, _getState, dependencies) =>
    !!dependencies.signInToProvider &&
    (await (dependencies.providerLoginAvailable?.() ?? true))
