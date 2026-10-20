import type { SubmissionDiagnostic } from "../../model"
import {
  canSendAgain,
  discardFailedTurn,
  restoreFailedTurn,
} from "../../application/usecases/send-draft"
import {
  prepareSubmission,
  preparedSubmissionFields,
  executionTargets,
  executionTargetConversation,
  permissionTargetConversation,
  questionTargetConversation,
  captureQuestionChoices,
  type PreparedSubmission,
  type ExecutionTarget,
  type PermissionTarget,
  type QuestionTarget,
  type QuestionChoice,
} from "@nessa/client"
import {
  type ApprovalMode,
  contentText,
  conversationHistoryEmpty,
  conversationSelectionOpen,
  MAX_SENT_PREVIEW_BYTES,
  messageFiles,
  storedImages,
  type CommandFailure,
  type ConversationSelection,
  type AttachmentResource,
  type MessageContent,
  type ReadFailure,
  type Turn,
  type UploadFailure,
} from "../../model"
import {
  createAsyncThunk,
  createSlice,
  type GetThunkAPI,
  type PayloadAction,
} from "@reduxjs/toolkit"
import type { LocalTabs } from "../../application/local-tabs"
import { emptyLocalTabs } from "../../application/local-tabs"
import {
  beginSend,
  reattemptSend,
  declineReason,
  draftMessage,
  failSend,
  refusalReleasesImages,
  submissionRefusalMessage,
  type SendOutcome,
} from "../../application/usecases/send-draft"
import { applyView } from "../../application/usecases/apply-view"
import { controlFailureMessage } from "../../application/usecases/control-failure"
import { boundSentPreviews } from "../../application/usecases/release-uploads"
import {
  AttachmentStagingError,
  ControlFailedError,
  ConversationUnavailableError,
  SubmissionRefusedError,
  type ConversationEffects,
  type UploadChange,
  type UploadedFile,
} from "../../application/ports"
import type { ConversationView } from "../../application/view"
import {
  restoreConversationTabs,
  type SavedConversationTabs,
} from "../../application/saved-tabs"
import { localConversationGateway as gateway } from "../gateway/local"
import { forgetDeleted } from "../../application/usecases"
import { conversationDeleted } from "./history"
import {
  PreparedOperationTable,
  type PreparedOperation,
} from "./prepared-operation-table"

type ThunkConfig = {
  state: { conversation: LocalTabs }
  extra: {
    conversation: ConversationEffects
    preparedOperations: PreparedOperationTable
    /**
     * Whether this surface can say where a file is, which decides what a
     * message refused over an unsendable file is told. Injected because the
     * conversation vertical does not talk to the host; composition answers it.
     */
    canChoosePaths: boolean
  }
}
export type SendDraftArg = {
  content: MessageContent
  id?: string
  /** Explicitly replace this refused local bubble, keeping fresh wire attempt IDs. */
  retryTurnId?: string
  steering?: boolean
  /**
   * What the caller knows about the session, which this slice cannot see. False
   * declines the draft here, with a reason, like every other decline. A caller
   * with no session to consult leaves it out, and the effects' own
   * `ConversationUnavailableError` is what then says so.
   */
  connected?: boolean
}
const detail = (error: unknown) =>
  error instanceof Error ? error.message : "The gateway request failed."
/** The sentence for a refused message: by its typed reason, else the error's own. */
const refusalDetail = (error: unknown) =>
  (error instanceof SubmissionRefusedError
    ? submissionRefusalMessage(error.reason)
    : undefined) ?? detail(error)
/** The same for a control, which has no draft to hand back and says so differently. */
const controlDetail = (error: unknown) =>
  (error instanceof ControlFailedError
    ? controlFailureMessage(error.reason, error.outcome)
    : undefined) ?? detail(error)

/**
 * Did this message go? A typed refusal is the gateway, or the client that
 * validates its arguments, saying it was not taken: the draft can come back.
 * A failure after admission was attempted proves nothing either way.
 *
 * Every pre-admission rejection arrives as that typed refusal, because the
 * effects port promises it: the gateway adapter translates one when the client
 * says a message was not taken, and `effects.test.ts` holds it to every code
 * the client decides that way. So there is no second reading of the client's
 * own `uncertain` here, and nothing in this file reads a wire answer at all.
 */
function sendOutcome(error: unknown, admissionAttempted: boolean): SendOutcome {
  if (error instanceof SubmissionRefusedError)
    return { kind: "refused", reupload: refusalReleasesImages(error.reason) }
  if (!admissionAttempted || error instanceof ConversationUnavailableError)
    return { kind: "refused", reupload: false }
  return { kind: "uncertain" }
}

// The typed reason behind that text, in the panel's own words, when the gateway
// gave one this build knows. Kept beside the message so a notice can say why
// without reading the words — never so a reader can infer what happened, which
// is the typed error's business and differs between a message and a control.
// A control can be certainly refused under a code with no word here, so this
// answering undefined is not the same as nothing being known about it.
const commandFailure = (error: unknown): CommandFailure | undefined =>
  error instanceof SubmissionRefusedError || error instanceof ControlFailedError
    ? error.reason
    : undefined

/**
 * Stop asked for a turn whose submit has not returned. The admission keeps
 * going; once it has, the same execution is stopped even if the tab was
 * closed locally in between.
 */
/** Bind a local tab without publishing a partial send transition. */
function bindServerConversation(
  tabs: LocalTabs,
  id: string,
  serverId: string,
): LocalTabs {
  const current = tabs.conversations.find((item) => item.id === id)
  if (!current || current.serverConversationId) return tabs
  return {
    ...tabs,
    conversations: tabs.conversations.map((item) =>
      item.id === id ? { ...item, serverConversationId: serverId } : item,
    ),
  }
}

/** Admission is known even if the separately requested stop acknowledgement fails. */
async function stopAfterAdmission(
  operation: PreparedOperation,
  id: string,
  { dispatch, extra }: Pick<GetThunkAPI<ThunkConfig>, "dispatch" | "extra">,
) {
  if (!operation.stopRequested) return
  try {
    await extra.conversation.stop(operation.prepared.target)
  } catch (error) {
    dispatch(
      showError({ id, message: controlDetail(error), failure: commandFailure(error) }),
    )
  }
}

/** Capture a tab and logical submission before awaiting any connection or admission. */
export const sendDraft = createAsyncThunk<void, SendDraftArg, ThunkConfig>(
  "conversation/sendDraft",
  async (input, { dispatch, getState, extra, rejectWithValue }) => {
    const tabs = getState().conversation
    const id = input.id ?? tabs.activeId
    const conv = tabs.conversations.find((item) => item.id === id)
    // The last way this could decline a draft and look like it had taken one:
    // the conversation closing in the same tick as a submit. Refused with a
    // reason like every other decline — silently, because a conversation that
    // is gone has nowhere to show a sentence.
    if (!conv) return rejectWithValue({ kind: "no-such-conversation" })
    // Every other local reason a draft is declined is decided in one pure
    // place and said in one place here, so "was this draft taken" has one
    // answer and not eight — the composer's full-pane editor rests on it.
    const decline = declineReason(conv, input, extra.canChoosePaths)
    if (decline) {
      if (decline.askAgain) void dispatch(refollowConversation(id))
      if (decline.message) dispatch(showError({ id, message: decline.message }))
      return rejectWithValue({ kind: decline.kind })
    }
    const content = draftMessage(conv, input.content)
    const text = contentText(content)
    const images = storedImages(content)
    const files = messageFiles(content)
    const serverId = conv.serverConversationId ?? crypto.randomUUID()
    const executionId = crypto.randomUUID()
    const actionId = crypto.randomUUID()
    let prepared: PreparedSubmission
    try {
      prepared = prepareSubmission({
        conversationId: serverId,
        executionId,
        requestId: actionId,
        mode: input.steering ? "steering" : "queued",
        text,
        attachments: images,
        files,
      })
    } catch (error) {
      dispatch(showError({ id, message: detail(error) }))
      return rejectWithValue({ kind: "invalid-submission" })
    }
    const operation: PreparedOperation = {
      prepared,
      stopRequested: false,
      inflight: true,
    }
    const operations = extra.preparedOperations
    operations.set(id, executionId, operation)
    // The operation exists before the one publication that makes it stoppable.
    // Subscribers can synchronously Stop/close; this promise still owns it (PS25–26).
    dispatch(
      submissionStarted({
        content,
        conversationId: id,
        prepared,
        retryTurnId: input.retryTurnId,
      }),
    )
    let admissionAttempted = false
    try {
      const created = await extra.conversation.create(serverId, conv.selection)
      if (created.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(conversationReady(id))
      admissionAttempted = true
      const receipt = await extra.conversation.submit(prepared)
      if (
        receipt.conversationId !== prepared.conversationId ||
        receipt.executionId !== executionId
      )
        throw new Error("Gateway returned a different submission identity.")
      dispatch(submissionAccepted({ id, executionId }))
      await stopAfterAdmission(operation, id, { dispatch, extra })
      operation.inflight = false
      operations.delete(id, executionId)
      await dispatch(refollowConversation(id))
    } catch (error) {
      operation.inflight = false
      if (sendOutcome(error, admissionAttempted).kind !== "uncertain")
        operations.delete(id, executionId)
      dispatch(
        submissionFailed({
          id,
          executionId,
          message: refusalDetail(error),
          outcome: sendOutcome(error, admissionAttempted),
          failure: commandFailure(error),
          diagnostic: {
            message: detail(error),
            code: error instanceof SubmissionRefusedError ? error.code : undefined,
            agent: error instanceof SubmissionRefusedError ? error.agent : undefined,
          },
        }),
      )
      throw error
    }
  },
)

/**
 * Send Again on a refused message. It always sends the draft, which owns the
 * message's attachments. A draft left empty (an older refusal offered after a
 * newer one was sent again) first gets the message back; one whose files must
 * be uploaded again waits in the composer for the next Send Again.
 */
export const retryFailedTurn = createAsyncThunk<
  void,
  { id: string; turnId: string },
  ThunkConfig
>("conversation/retryFailedTurn", async ({ id, turnId }, { dispatch, getState }) => {
  dispatch(failedTurnRestored({ id, turnId }))
  const conv = getState().conversation.conversations.find((item) => item.id === id)
  // Restored images upload first; Send Again is pressed again once they have.
  if (!conv || conv.draft.length === 0 || !canSendAgain(conv, turnId)) return
  await dispatch(sendDraft({ id, content: conv.draft, retryTurnId: turnId }))
})

/**
 * Put one draft image's original bytes on the gateway, and record how that went
 * on the file itself — including what the gateway stored them as, which is what
 * a message will name and may be nothing like the file.
 *
 * Runs at attach time, so it is also what first creates the gateway conversation
 * for a new tab: `attachment.begin` needs one to upload into. It never throws —
 * every ending is an upload state on the tile — and it changes only a file
 * still in a draft, so an upload that settles after its tile was removed
 * changes nothing.
 */
/**
 * Ask for one step of a file's upload state, and say whether it was taken.
 *
 * The rules refuse a step that does not start where the file is, by leaving
 * the tabs exactly as they were. Whoever asked has to know, because the next
 * thing it does is send bytes.
 */
export const takeUploadStep =
  (change: UploadChange) =>
  (
    dispatch: (action: ReturnType<typeof uploadChanged>) => unknown,
    getState: () => ThunkConfig["state"],
  ): boolean => {
    const before = getState().conversation
    dispatch(uploadChanged(change))
    return getState().conversation !== before
  }

export const stageAttachment = createAsyncThunk<
  void,
  { id: string; placementId: string; file: UploadedFile; bytes: Blob },
  ThunkConfig
>(
  "conversation/stageAttachment",
  async (input, { dispatch, getState, extra, signal }) => {
    const target = { conversationId: input.id, placementId: input.placementId }
    const current = getState().conversation.conversations.find(
      (item) => item.id === target.conversationId,
    )
    if (
      !current?.draft.some(
        (part) => part.type === "file" && part.placementId === target.placementId,
      )
    )
      return
    const serverId = current.serverConversationId ?? crypto.randomUUID()
    dispatch(bindConversation({ id: target.conversationId, serverId }))
    try {
      const created = await extra.conversation.create(serverId, current.selection)
      if (created.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(conversationReady(target.conversationId))
      // `signal` is this thunk's own: aborting the dispatched promise stops the
      // upload, which is how a removed tile gives its slot back.
      const image = await extra.conversation.stageAttachment(
        serverId,
        input.file,
        input.bytes,
        signal,
      )
      dispatch(
        uploadChanged({
          ...target,
          to: "stored",
          image,
        }),
      )
    } catch (error) {
      // Only a staging refusal says the gateway looked at these bytes and said
      // no, and a conversation deleted, here or elsewhere, stays deleted.
      // Anything else — not connected, conversation not created, no answer — is
      // the gateway being away, and may work if tried again.
      const reason: UploadFailure =
        error instanceof AttachmentStagingError
          ? error.reason
          : error instanceof SubmissionRefusedError &&
              error.reason === "conversation-deleted"
            ? "conversation-deleted"
            : "unavailable"
      dispatch(
        uploadChanged({
          ...target,
          to: "failed",
          reason,
        }),
      )
    }
  },
)

/**
 * Close a tab, and the gateway conversation with it when this window made that
 * conversation only to upload into and nothing was ever said in it.
 *
 * Attaching an image creates the gateway conversation, because an upload needs
 * one. A conversation left open keeps the staged bytes, and closing it on the
 * gateway is what releases them. It is only done when a view has shown the
 * conversation to be empty and idle: a tab with turns, or one whose view has
 * not arrived, may be somebody's work — this window's or another surface's —
 * and closing a tab never stops that.
 *
 * The tab closes first and whatever the gateway says. A release that fails is
 * survivable — what the conversation still holds is the gateway's to account
 * for, and nothing here depends on it being gone — and is reported, not shown.
 */
export const closeTab = createAsyncThunk<void, string, ThunkConfig>(
  "conversation/closeTab",
  async (id, { dispatch, getState, extra }) => {
    const current = getState().conversation.conversations.find((item) => item.id === id)
    dispatch(closeConversation(id))
    const remote = current?.remote
    if (
      !current?.serverConversationId ||
      !conversationHistoryEmpty(current) ||
      !remote ||
      remote.running ||
      remote.pending.length > 0
    )
      return
    try {
      await extra.conversation.close(current.serverConversationId)
    } catch (error) {
      console.warn("[nessa] an emptied conversation was not closed on the gateway", error)
    }
  },
)

/**
 * The follow each tab has open, by tab id, a single read included: what stops
 * it, and whether it is a single read. Outside the state, which holds only
 * what is serializable; `readRequest` names the follow whose views apply. One
 * table per store (its thunk dependencies), so two stores never see each
 * other's follows. The one place a follow is replaced: a new follow of a tab
 * stops the one it had, which settles that one's caller (P8).
 */
type HeldFollow = { stop: () => void; once: boolean }
const followTables = new WeakMap<ThunkConfig["extra"], Map<string, HeldFollow>>()
function followsOf(extra: ThunkConfig["extra"]): Map<string, HeldFollow> {
  let table = followTables.get(extra)
  if (!table) followTables.set(extra, (table = new Map()))
  return table
}

type FollowApi = Pick<
  GetThunkAPI<ThunkConfig>,
  "dispatch" | "getState" | "extra" | "requestId"
>

/**
 * Follows a tab's conversation under `requestId`: every view the gateway
 * sends while that is the tab's `readRequest` applies. Settles once the first
 * view or failure is applied, or the follow is stopped first. Either kind
 * replaces the follow the tab had, stopping it; a tab's follow (`once` false)
 * goes on until stopped, and a single read (`once` true) stops itself after
 * its first word.
 */
async function followTab(id: string, once: boolean, api: FollowApi): Promise<void> {
  const { dispatch, getState, extra, requestId } = api
  const current = getState().conversation.conversations.find((item) => item.id === id)
  if (!current?.serverConversationId) return
  const serverId = current.serverConversationId
  const follows = followsOf(extra)
  follows.get(id)?.stop()
  dispatch(readStarted({ id, requestId }))
  await new Promise<void>((settled) => {
    let stopping = () => {}
    // The last view this follow applied, whole: one that says nothing new is
    // not applied again, so what is on screen keeps its references.
    let applied: string | undefined
    const told = () => {
      settled()
      if (once) stopping()
    }
    const stop = extra.conversation.follow(serverId, {
      view: (view) => {
        if (view.conversationId !== serverId) {
          // A gateway answering about a different conversation: a view the
          // panel cannot use, which the word alone reports as a stale one.
          console.warn(
            "[nessa] a conversation was not refreshed",
            "unavailable",
            new Error("Gateway returned a different conversation identity."),
          )
          dispatch(readFailed({ id, requestId, reason: "unavailable" }))
          applied = undefined
        } else {
          const key = JSON.stringify(view)
          if (key !== applied) dispatch(viewReceived({ id, requestId, serverId, view }))
          applied = key
        }
        told()
      },
      failed: (reason, cause) => {
        // The tab keeps the word; this keeps what it was translated from.
        // Reported every time rather than once, because the cause behind one
        // word can change between attempts.
        console.warn("[nessa] a conversation was not refreshed", reason, cause)
        dispatch(readFailed({ id, requestId, reason }))
        // The word stays until a view applies: the next one does, even unchanged.
        applied = undefined
        told()
      },
    })
    let stopped = false
    stopping = () => {
      if (stopped) return
      stopped = true
      stop()
      if (follows.get(id)?.stop === stopping) follows.delete(id)
      settled()
    }
    follows.set(id, { stop: stopping, once })
  })
}

/**
 * Follows a tab's conversation: every view the gateway sends while this
 * follow is the tab's (`readRequest`) applies. Following again replaces the
 * tab's follow, and the new one's first view is read after this call, so
 * after any command answered before it. Settles once the first view or
 * failure is applied, or the follow is stopped first; the follow goes on.
 */
export const followConversation = createAsyncThunk<void, string, ThunkConfig>(
  "conversation/follow",
  (id, api) => followTab(id, false, api),
)

/**
 * After a command, its result is read: a tab that is followed is followed
 * again, and one nobody follows (not on screen) is read once. Either way the
 * view is read after the command was answered, and one read before cannot
 * apply after it (`readRequest`, which the command let go). Only the tab on
 * screen is left following.
 */
export const refollowConversation = createAsyncThunk<void, string, ThunkConfig>(
  "conversation/refollow",
  (id, api) => followTab(id, followsOf(api.extra).get(id)?.once ?? true, api),
)

/** Stops following a tab's conversation: nothing it says applies after. */
export const unfollowConversation = createAsyncThunk<void, string, ThunkConfig>(
  "conversation/unfollow",
  async (id, { dispatch, extra }) => {
    followsOf(extra).get(id)?.stop()
    dispatch(invalidateRead(id))
  },
)

export type Control =
  | { kind: "setApprovalMode"; mode: ApprovalMode }
  | { kind: "close" }
  | { kind: "reorder"; executionIds: string[] }
  | { kind: "remove"; target: ExecutionTarget }
  | { kind: "answer"; target: PermissionTarget; optionId: string }
  | {
      kind: "answerQuestion"
      target: QuestionTarget
      /** Null declines: the agent is told it was answered with nothing. */
      choices: readonly QuestionChoice[] | null
    }
  | { kind: "cancel"; target: PermissionTarget }
  | { kind: "stop"; target: ExecutionTarget }
  | { kind: "retry"; target: ExecutionTarget }
/** Capture the deliberate command before a creation/retry can suspend it (PS21). */
function captureControl(source: Control): Control {
  const command = { ...source }
  if (command.kind === "reorder") {
    const executionIds: string[] = []
    for (let index = 0; index < command.executionIds.length; index++)
      executionIds.push(command.executionIds[index])
    command.executionIds = executionIds
  }
  if (command.kind === "answerQuestion")
    command.choices = captureQuestionChoices(command.choices)
  return Object.freeze(command)
}

/** Turns this gateway's usage-limit continuation preference on or off. */
export const setUsageLimitAutoContinue = createAsyncThunk<void, boolean, ThunkConfig>(
  "conversation/setUsageLimitAutoContinue",
  async (enabled, { extra }) => {
    await extra.conversation.setUsageLimitAutoContinue(enabled)
  },
)

export const controlConversation = createAsyncThunk<
  void,
  { id: string; control: Control },
  ThunkConfig
>(
  "conversation/control",
  async ({ id, control: source }, { dispatch, getState, extra }) => {
    const control = captureControl(source)
    const current = getState().conversation.conversations.find((item) => item.id === id)
    if (!current?.serverConversationId) return
    const serverId = current.serverConversationId
    // A delayed action belongs to the conversation captured on the approval
    // card, even if its local tab has since been rebound. No create/control
    // runs for that other conversation (permission-targets tests, PT4).
    if (
      (control.kind === "answer" || control.kind === "cancel") &&
      permissionTargetConversation(control.target) !== serverId
    )
      return
    if (
      control.kind === "answerQuestion" &&
      questionTargetConversation(control.target) !== serverId
    )
      return
    if (
      (control.kind === "stop" ||
        control.kind === "remove" ||
        control.kind === "retry") &&
      executionTargetConversation(control.target) !== serverId
    )
      return
    if (control.kind === "stop") {
      const operation = extra.preparedOperations.get(id, control.target.executionId)
      if (operation) operation.stopRequested = true
      // Cancellation can overtake creation or a retry; it never owns the ordinary
      // control busy state (PS6/PS12). The gateway resolves current authority.
      try {
        await extra.conversation.stop(control.target)
      } catch (error) {
        dispatch(
          showError({
            id,
            message: controlDetail(error),
            failure: commandFailure(error),
          }),
        )
        throw error
      } finally {
        await dispatch(refollowConversation(id))
      }
      return
    }
    if (current.controlPending) return
    const retryOperation =
      control.kind === "retry"
        ? extra.preparedOperations.get(id, control.target.executionId)
        : undefined
    // Inflight intent stays available to Stop; only an unknown receipt permits retry.
    if (
      control.kind === "retry" &&
      (!retryOperation ||
        !current.turns.some(
          (turn) =>
            turn.from === "user" &&
            turn.executionId === control.target.executionId &&
            turn.receipt === "unknown",
        ))
    )
      return
    const requestedOrder = control.kind === "reorder" ? control.executionIds : []
    // Retry owns its prepared value across creation too: an authoritative view
    // can settle the old receipt while create is waiting, but Stop must still
    // be able to mark this promise for a second Stop after admission.
    if (retryOperation) retryOperation.inflight = true
    try {
      dispatch(controlStarted(id))
      if (control.kind === "close")
        dispatch(cancellationChanged({ id, status: "cancelling" }))
      const created = await extra.conversation.create(serverId, current.selection)
      if (created.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(conversationReady(id))
      switch (control.kind) {
        case "setApprovalMode":
          await extra.conversation.setApprovalMode(serverId, control.mode)
          break
        case "close":
          await extra.conversation.close(serverId)
          dispatch(cancellationChanged({ id, status: "cancelled" }))
          // Closing releases every file the conversation was keeping, so the
          // draft's stored images name nothing now. Back to not-started they go,
          // and the panel uploads them again before the next send.
          dispatch(uploadsReleased(id))
          break
        case "reorder": {
          const result = await extra.conversation.reorder(serverId, requestedOrder)
          if (result === "queue_changed")
            dispatch(
              showError({
                id,
                message:
                  "The queue changed before the move. Its current order has been refreshed.",
              }),
            )
          if (result === "priority_conflict")
            dispatch(
              showError({
                id,
                message: "Steering messages must stay ahead of ordinary queued messages.",
              }),
            )
          break
        }
        case "remove":
          await extra.conversation.remove(control.target)
          break
        case "answer":
          await extra.conversation.answer(control.target, control.optionId)
          break
        case "answerQuestion":
          await extra.conversation.answerQuestion(control.target, control.choices)
          break
        case "cancel":
          await extra.conversation.cancel(control.target)
          break
        case "retry": {
          const executionId = control.target.executionId
          const operation = retryOperation
          if (!operation) throw new Error("Retry intent was not captured.")
          dispatch(submissionReattempted({ id, executionId }))
          const receipt = await extra.conversation.submit(operation.prepared)
          if (
            receipt.conversationId !== operation.prepared.conversationId ||
            receipt.executionId !== executionId
          )
            throw new Error("Gateway returned a different submission identity.")
          dispatch(submissionAccepted({ id, executionId }))
          await stopAfterAdmission(operation, id, { dispatch, extra })
          break
        }
      }
      await dispatch(refollowConversation(id))
    } catch (error) {
      if (control.kind === "close") {
        dispatch(cancellationChanged({ id }))
        // The close may have applied — a lost acknowledgement, or a gateway that
        // closed and then could not finish cleaning up. Forgetting is safe either
        // way: uploading bytes the conversation still holds answers `stored`
        // without sending them again.
        dispatch(uploadsReleased(id))
      }
      // A lost acknowledgement may follow an applied control. Follow authority
      // again; never replay the control or infer that the previous order still holds.
      await dispatch(refollowConversation(id))
      // Every control, retry included. A retry does re-send a message, but it
      // does not act on a refusal the way `sendDraft` does — the turn keeps its
      // receipt and the draft is untouched — so a message's sentences, which
      // promise the draft is back with its images, would describe something that
      // did not happen here.
      dispatch(
        showError({ id, message: controlDetail(error), failure: commandFailure(error) }),
      )
      throw error
    } finally {
      if (retryOperation) {
        retryOperation.inflight = false
        extra.preparedOperations.prune(getState().conversation.conversations)
      }
      dispatch(controlFinished(id))
    }
  },
)
function capturedTurn(turns: readonly Turn[]): string | undefined {
  const running = [...turns]
    .reverse()
    .find(
      (turn) =>
        turn.from === "assistant" && turn.status === "running" && turn.executionId,
    )
  if (running?.executionId) return running.executionId
  for (const turn of [...turns].reverse()) {
    if (turn.from !== "user" || !turn.executionId) continue
    if (
      turn.receipt === "sending" ||
      turn.receipt === "accepted" ||
      turn.receipt === "queued" ||
      turn.receipt === "unknown"
    )
      return turn.executionId
  }
  return undefined
}
export const stopGenerating = createAsyncThunk<
  void,
  { conversationId?: string } | undefined,
  ThunkConfig
>("conversation/stop", async (input, { dispatch, getState, extra }) => {
  const id = input?.conversationId ?? getState().conversation.activeId
  const current = getState().conversation.conversations.find((item) => item.id === id)
  const executionId = current ? capturedTurn(current.turns) : undefined
  if (!executionId) return
  const operation = extra.preparedOperations.get(id, executionId)
  const target =
    operation?.prepared.target ??
    (current?.serverConversationId
      ? executionTargets({
          conversationId: current.serverConversationId,
          messages: current.turns.flatMap((turn) =>
            turn.executionId ? [{ executionId: turn.executionId }] : [],
          ),
          pending: current.remote?.pending ?? [],
        }).find((target) => target.executionId === executionId)
      : undefined)
  if (!target) return
  await dispatch(controlConversation({ id, control: { kind: "stop", target } })).unwrap()
})

const conversationSlice = createSlice({
  name: "conversation",
  initialState: emptyLocalTabs(),
  reducers: {
    restoreConversations(state, action: PayloadAction<SavedConversationTabs>) {
      return restoreConversationTabs(state, action.payload)
    },
    cancellationChanged(
      state,
      action: PayloadAction<{ id: string; status?: "cancelling" | "cancelled" }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current) current.cancellationStatus = action.payload.status
    },
    renameConversation(state, action: PayloadAction<{ id: string; title: string }>) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      const title = action.payload.title.trim().slice(0, 120)
      if (current && title) {
        current.title = title
        current.titleEdited = true
      }
    },
    attachFiles(
      state,
      action: PayloadAction<{ files: AttachmentResource[]; conversationId: string }>,
    ) {
      return gateway.attachFiles(
        state,
        action.payload.files,
        action.payload.conversationId,
      )
    },
    removeFile(state, action: PayloadAction<string>) {
      return gateway.removeFile(state, action.payload)
    },
    uploadChanged(state, action: PayloadAction<UploadChange>) {
      return gateway.changeUpload(state, action.payload)
    },
    uploadsReleased(state, action: PayloadAction<string>) {
      return gateway.forgetStoredUploads(state, action.payload)
    },
    moveActive(state, action: PayloadAction<-1 | 1>) {
      return gateway.moveActive(state, action.payload)
    },
    setActive(state, action: PayloadAction<string>) {
      return gateway.setActive(state, action.payload)
    },
    setDraft(state, action: PayloadAction<{ draft: MessageContent; id?: string }>) {
      return gateway.setDraft(state, action.payload)
    },
    setSelection(
      state,
      action: PayloadAction<{ id: string; selection: ConversationSelection }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current && conversationSelectionOpen(current)) {
        current.selection = action.payload.selection
      }
    },
    openConversation(state) {
      return gateway.openConversation(state)
    },
    chooseModel(state, action: PayloadAction<ConversationSelection>) {
      return gateway.chooseModel(state, action.payload)
    },
    openListed(
      state,
      action: PayloadAction<{ serverConversationId: string; title: string | null }>,
    ) {
      return gateway.openListed(state, action.payload)
    },
    closeConversation(state, action: PayloadAction<string>) {
      return gateway.closeConversation(state, action.payload)
    },
    bindConversation(state, action: PayloadAction<{ id: string; serverId: string }>) {
      return bindServerConversation(state, action.payload.id, action.payload.serverId)
    },
    conversationReady(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) current.serverReady = true
    },
    submissionStarted(
      state,
      action: PayloadAction<{
        content: MessageContent
        conversationId: string
        prepared: PreparedSubmission
        retryTurnId?: string
      }>,
    ) {
      const { content, conversationId } = action.payload
      const prepared = preparedSubmissionFields(action.payload.prepared)
      return beginSend(
        bindServerConversation(state, conversationId, prepared.conversationId),
        {
          content,
          conversationId,
          executionId: prepared.executionId,
          actionId: prepared.requestId,
          mode: prepared.mode,
          retryTurnId: action.payload.retryTurnId,
        },
      )
    },
    submissionReattempted(
      state,
      action: PayloadAction<{ id: string; executionId: string }>,
    ) {
      return reattemptSend(state, action.payload.id, action.payload.executionId)
    },
    submissionAccepted(
      state,
      action: PayloadAction<{ id: string; executionId: string }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      const turn = current?.turns.find(
        (turn) => turn.from === "user" && turn.executionId === action.payload.executionId,
      )
      if (turn?.from === "user" && turn.receipt !== "delivered") {
        turn.receipt = "accepted"
        turn.error = undefined
      }
      if (current) current.readRequest = undefined
    },
    failedTurnDiscarded(state, action: PayloadAction<{ id: string; turnId: string }>) {
      return discardFailedTurn(state, action.payload.id, action.payload.turnId)
    },
    failedTurnRestored(state, action: PayloadAction<{ id: string; turnId: string }>) {
      return restoreFailedTurn(state, action.payload.id, action.payload.turnId)
    },
    submissionFailed(
      state,
      action: PayloadAction<{
        id: string
        executionId: string
        message: string
        outcome: SendOutcome
        diagnostic?: SubmissionDiagnostic
        failure?: CommandFailure
      }>,
    ) {
      return failSend(
        state,
        action.payload.id,
        action.payload.executionId,
        action.payload.message,
        action.payload.outcome,
        action.payload.failure,
        action.payload.diagnostic,
      )
    },
    showError(
      state,
      action: PayloadAction<{ id: string; message: string; failure?: CommandFailure }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current) {
        current.error = action.payload.message
        current.failure = action.payload.failure
        current.errorTurn = undefined
      }
    },
    readStarted(state, action: PayloadAction<{ id: string; requestId: string }>) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current) current.readRequest = action.payload.requestId
    },
    invalidateRead(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) current.readRequest = undefined
    },
    readFailed(
      state,
      action: PayloadAction<{ id: string; requestId: string; reason: ReadFailure }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      // The follow goes on: a later view clears the word.
      if (current?.readRequest === action.payload.requestId)
        current.readError = action.payload.reason
    },
    viewReceived(
      state,
      action: PayloadAction<{
        id: string
        requestId: string
        serverId: string
        view: ConversationView
      }>,
    ) {
      const { id, requestId, serverId, view } = action.payload
      const index = state.conversations.findIndex((item) => item.id === id)
      const current = state.conversations[index]
      if (
        current?.readRequest === requestId &&
        current.serverConversationId === serverId
      ) {
        // Every view applies: the gateway sends one when what a person sees
        // changes, which its revision alone does not say (a permission ask,
        // an app's review, the agent's lifecycle). The follow stays the tab's.
        state.conversations[index] = {
          ...applyView(current, view),
          readRequest: requestId,
        }
      }
    },
    controlStarted(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) {
        current.controlPending = true
        current.readRequest = undefined
        current.error = undefined
        current.failure = undefined
        current.errorTurn = undefined
      }
    },
    controlFinished(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) current.controlPending = false
    },
  },
  extraReducers: (builder) => {
    builder.addCase(conversationDeleted, (state, action) =>
      forgetDeleted(state, action.payload),
    )
  },
})
export const {
  restoreConversations,
  cancellationChanged,
  renameConversation,
  attachFiles,
  removeFile,
  uploadChanged,
  uploadsReleased,
  setActive,
  moveActive,
  setDraft,
  setSelection,
  openConversation,
  chooseModel,
  openListed,
  closeConversation,
  bindConversation,
  conversationReady,
  submissionStarted,
  submissionAccepted,
  submissionReattempted,
  submissionFailed,
  failedTurnDiscarded,
  failedTurnRestored,
  showError,
  readStarted,
  invalidateRead,
  readFailed,
  viewReceived,
  controlStarted,
  controlFinished,
} = conversationSlice.actions
/** Reconcile all reference-changing publications through the one preview bound. */
export const conversationReducer: typeof conversationSlice.reducer = (state, action) => {
  const next = conversationSlice.reducer(state, action)
  return next.conversations === state?.conversations
    ? next
    : boundSentPreviews(next, MAX_SENT_PREVIEW_BYTES)
}
// Mounted beside the tabs by the store, which reaches this vertical through this module alone.
export { conversationHistoryReducer } from "./history"

// Narrow composition entry: the store mounts reducers without loading UI barrels.
export { attachmentResourceUsage } from "../../application/queries/attachment-resources"
export { canSendAgain } from "../../application/usecases/send-draft"

export { AttachmentResource } from "../../model"
