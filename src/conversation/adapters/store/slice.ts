import {
  contentText,
  MAX_SENT_PREVIEW_BYTES,
  messageImages,
  storedImages,
  type CommandFailure,
  type FileAttachment,
  type MessageContent,
  type ReadFailure,
  type UploadFailure,
} from "../../model"
import {
  createAsyncThunk,
  createSlice,
  current as snapshot,
  type PayloadAction,
} from "@reduxjs/toolkit"
import type { LocalTabs } from "../../application/local-tabs"
import { emptyLocalTabs } from "../../application/local-tabs"
import {
  beginSend,
  declineReason,
  draftMessage,
  failSend,
  imageRefusalMessage,
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
  ConversationReadFailedError,
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

type ThunkConfig = {
  state: { conversation: LocalTabs }
  extra: { conversation: ConversationEffects }
}
export type SendDraftArg = {
  content: MessageContent
  id?: string
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

// The same for a read, which always has a word: the effects port promises one
// for every rejected read, and this file's own identity check — a view answering
// about another conversation — is a view the panel cannot use either. Nothing
// here reads a wire code or a sentence.
const readFailure = (error: unknown): ReadFailure =>
  error instanceof ConversationReadFailedError ? error.reason : "unavailable"

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
    const decline = declineReason(conv, input)
    if (decline) {
      if (decline.askAgain) void dispatch(refreshConversation(id))
      if (decline.message) dispatch(showError({ id, message: decline.message }))
      return rejectWithValue({ kind: decline.kind })
    }
    const content = draftMessage(conv, input.content)
    const text = contentText(content)
    const images = storedImages(content)
    const serverId = conv.serverConversationId ?? crypto.randomUUID()
    const executionId = crypto.randomUUID()
    const actionId = crypto.randomUUID()
    dispatch(bindConversation({ id, serverId }))
    dispatch(
      submissionStarted({
        content,
        conversationId: id,
        executionId,
        actionId,
        mode: input.steering ? "steering" : "queued",
      }),
    )
    let admissionAttempted = false
    try {
      const created = await extra.conversation.create(serverId)
      if (created.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(conversationReady(id))
      const submission = {
        conversationId: serverId,
        executionId,
        actionId,
        text,
        attachments: images,
      }
      admissionAttempted = true
      const receipt = await (input.steering
        ? extra.conversation.steer(submission)
        : extra.conversation.send(submission))
      if (receipt.executionId !== executionId)
        throw new Error("Gateway returned a different submission identity.")
      dispatch(submissionAccepted({ id, executionId }))
      await dispatch(refreshConversation(id))
    } catch (error) {
      dispatch(
        submissionFailed({
          id,
          executionId,
          message: refusalDetail(error),
          outcome: sendOutcome(error, admissionAttempted),
          failure: commandFailure(error),
        }),
      )
      throw error
    }
  },
)

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
  { id: string; fileId: string; file: UploadedFile; bytes: Blob },
  ThunkConfig
>(
  "conversation/stageAttachment",
  async (input, { dispatch, getState, extra, signal }) => {
    const current = getState().conversation.conversations.find(
      (item) => item.id === input.id,
    )
    if (!current?.draft.some((part) => part.type === "file" && part.id === input.fileId))
      return
    const serverId = current.serverConversationId ?? crypto.randomUUID()
    dispatch(bindConversation({ id: input.id, serverId }))
    try {
      const created = await extra.conversation.create(serverId)
      if (created.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(conversationReady(input.id))
      // `signal` is this thunk's own: aborting the dispatched promise stops the
      // upload, which is how a removed tile gives its slot back.
      const image = await extra.conversation.stageAttachment(
        serverId,
        input.file,
        input.bytes,
        signal,
      )
      dispatch(uploadChanged({ fileId: input.fileId, to: "stored", image }))
    } catch (error) {
      // Only a staging refusal says the gateway looked at these bytes and said
      // no. Anything else — not connected, conversation not created, no answer —
      // is the gateway being away, and may work if tried again.
      const reason: UploadFailure =
        error instanceof AttachmentStagingError ? error.reason : "unavailable"
      dispatch(uploadChanged({ fileId: input.fileId, to: "failed", reason }))
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
      current.turns.length > 0 ||
      !remote ||
      remote.truncated ||
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

/** Read once. Callers control polling lifetime; reducers reject older in-flight reads. */
export const refreshConversation = createAsyncThunk<void, string, ThunkConfig>(
  "conversation/refresh",
  async (id, { dispatch, getState, extra, requestId }) => {
    const current = getState().conversation.conversations.find((item) => item.id === id)
    if (!current?.serverConversationId) return
    const serverId = current.serverConversationId
    dispatch(readStarted({ id, requestId }))
    try {
      const view = await extra.conversation.read(serverId)
      if (view.conversationId !== serverId)
        throw new Error("Gateway returned a different conversation identity.")
      dispatch(viewReceived({ id, requestId, serverId, view }))
    } catch (error) {
      const reason = readFailure(error)
      // The tab keeps the word; this keeps what it was translated from. Two of
      // these have no other way out: a code this build has no name for, and the
      // identity check above — a gateway answering about a different
      // conversation — which the word alone reports as an ordinary stale view.
      // Reported every time rather than once, because each poll is a separate
      // request and the cause behind one word can change between them.
      console.warn("[nessa] a conversation was not refreshed", reason, error)
      dispatch(readFailed({ id, requestId, reason }))
    }
  },
)

export type Control =
  | { kind: "close" }
  | { kind: "reorder"; executionIds: string[] }
  | { kind: "remove"; executionId: string }
  | { kind: "answer"; executionId: string; permissionId: string; optionId: string }
  | { kind: "cancel"; executionId: string; permissionId: string }
  | { kind: "retry"; executionId: string }
export const controlConversation = createAsyncThunk<
  void,
  { id: string; control: Control },
  ThunkConfig
>("conversation/control", async ({ id, control }, { dispatch, getState, extra }) => {
  const current = getState().conversation.conversations.find((item) => item.id === id)
  if (!current?.serverConversationId || current.controlPending) return
  const serverId = current.serverConversationId
  const requestedOrder = control.kind === "reorder" ? [...control.executionIds] : []
  dispatch(controlStarted(id))
  if (control.kind === "close")
    dispatch(cancellationChanged({ id, status: "cancelling" }))
  try {
    const created = await extra.conversation.create(serverId)
    if (created.conversationId !== serverId)
      throw new Error("Gateway returned a different conversation identity.")
    dispatch(conversationReady(id))
    switch (control.kind) {
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
        await extra.conversation.remove(serverId, control.executionId)
        break
      case "answer":
        await extra.conversation.answer(
          serverId,
          control.executionId,
          control.permissionId,
          control.optionId,
        )
        break
      case "cancel":
        await extra.conversation.cancel(
          serverId,
          control.executionId,
          control.permissionId,
        )
        break
      case "retry": {
        const turn = current.turns.find(
          (turn) => turn.from === "user" && turn.executionId === control.executionId,
        )
        if (!turn || turn.from !== "user" || !turn.actionId || turn.receipt !== "unknown")
          return
        // The turn's content has not changed since it was sent, so this names
        // the same images: one execution ID stays one message.
        const images = messageImages(turn.content)
        if (!images.ok) {
          // Not reachable from a turn this window sent — it only became a turn
          // because its images could go. Said rather than skipped all the same.
          dispatch(showError({ id, message: imageRefusalMessage(images.refusal) }))
          return
        }
        const input = {
          conversationId: serverId,
          executionId: control.executionId,
          actionId: turn.actionId,
          text: contentText(turn.content),
          attachments: images.images,
        }
        const receipt = await (turn.mode === "steering"
          ? extra.conversation.steer(input)
          : extra.conversation.send(input))
        if (receipt.executionId !== control.executionId)
          throw new Error("Gateway returned a different submission identity.")
        dispatch(submissionAccepted({ id, executionId: control.executionId }))
        break
      }
    }
    await dispatch(refreshConversation(id))
  } catch (error) {
    if (control.kind === "close") {
      dispatch(cancellationChanged({ id }))
      // The close may have applied — a lost acknowledgement, or a gateway that
      // closed and then could not finish cleaning up. Forgetting is safe either
      // way: uploading bytes the conversation still holds answers `stored`
      // without sending them again.
      dispatch(uploadsReleased(id))
    }
    // A lost acknowledgement may follow an applied control. Read authority again;
    // never replay the control or infer that the previous order still holds.
    await dispatch(refreshConversation(id))
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
    dispatch(controlFinished(id))
  }
})
export const stopGenerating = createAsyncThunk<
  void,
  { conversationId?: string } | undefined,
  ThunkConfig
>("conversation/stop", async (input, { dispatch, getState }) => {
  await dispatch(
    controlConversation({
      id: input?.conversationId ?? getState().conversation.activeId,
      control: { kind: "close" },
    }),
  )
})

/**
 * Apply the sent-preview bound to a draft state these reducers have already
 * changed. The rule itself is pure and returns new tabs; a reducer that has
 * touched its draft may not also return a value, so the result is written back.
 */
function releaseOldPreviews(state: LocalTabs) {
  const bounded = boundSentPreviews(snapshot(state), MAX_SENT_PREVIEW_BYTES)
  state.conversations = bounded.conversations
}

const conversationSlice = createSlice({
  name: "conversation",
  initialState: emptyLocalTabs(),
  reducers: {
    restoreConversations(_state, action: PayloadAction<SavedConversationTabs>) {
      return restoreConversationTabs(action.payload)
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
      action: PayloadAction<{ files: FileAttachment[]; conversationId: string }>,
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
    openConversation(state) {
      return gateway.openConversation(state)
    },
    closeConversation(state, action: PayloadAction<string>) {
      return gateway.closeConversation(state, action.payload)
    },
    bindConversation(state, action: PayloadAction<{ id: string; serverId: string }>) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current && !current.serverConversationId)
        current.serverConversationId = action.payload.serverId
    },
    conversationReady(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) current.serverReady = true
    },
    submissionStarted(state, action: PayloadAction<Parameters<typeof beginSend>[1]>) {
      return beginSend(state, action.payload)
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
      // The gateway has the message: its originals are only previews from here.
      releaseOldPreviews(state)
    },
    submissionFailed(
      state,
      action: PayloadAction<{
        id: string
        executionId: string
        message: string
        outcome: SendOutcome
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
      if (current?.readRequest === action.payload.requestId) {
        current.readError = action.payload.reason
        current.readRequest = undefined
      }
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
        if (current.revision === view.revision) {
          current.readRequest = undefined
          current.readError = undefined
        } else {
          state.conversations[index] = applyView(current, view)
          // A view can be what first says a turn was taken.
          releaseOldPreviews(state)
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
      }
    },
    controlFinished(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) current.controlPending = false
    },
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
  openConversation,
  closeConversation,
  bindConversation,
  conversationReady,
  submissionStarted,
  submissionAccepted,
  submissionFailed,
  showError,
  readStarted,
  invalidateRead,
  readFailed,
  viewReceived,
  controlStarted,
  controlFinished,
} = conversationSlice.actions
export const conversationReducer = conversationSlice.reducer
