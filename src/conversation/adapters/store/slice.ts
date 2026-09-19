import { NessaConversationMutationError } from "@nessa/client"
import {
  contentText,
  hasFileAttachments,
  type FileAttachment,
  type MessageContent,
} from "../../model"
import { createAsyncThunk, createSlice, type PayloadAction } from "@reduxjs/toolkit"
import type { LocalTabs } from "../../application/local-tabs"
import { emptyLocalTabs } from "../../application/local-tabs"
import { beginSend, failSend } from "../../application/usecases/send-draft"
import { applyView } from "../../application/usecases/apply-view"
import {
  ConversationUnavailableError,
  type ConversationEffects,
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
export type SendDraftArg = { content: MessageContent; id?: string; steering?: boolean }
const detail = (error: unknown) =>
  error instanceof Error ? error.message : "The gateway request failed."

/** Capture a tab and logical submission before awaiting any connection or admission. */
export const sendDraft = createAsyncThunk<void, SendDraftArg, ThunkConfig>(
  "conversation/sendDraft",
  async (input, { dispatch, getState, extra, rejectWithValue }) => {
    const tabs = getState().conversation
    const id = input.id ?? tabs.activeId
    const current = tabs.conversations.find((item) => item.id === id)
    // The last way this could decline a draft and look like it had taken one:
    // the conversation closing in the same tick as a submit. Refused with a
    // reason like every other decline, so "was this draft taken" has one answer
    // and not two — the composer's full-pane editor rests on it.
    if (!current) return rejectWithValue({ kind: "no-such-conversation" })
    if (hasFileAttachments(input.content) || hasFileAttachments(current.draft)) {
      dispatch(
        showError({
          id,
          message:
            "File attachments are preview-only. Remove them before sending; this provider accepts text.",
        }),
      )
      return rejectWithValue({ kind: "preview-only-files" })
    }
    const text = contentText(input.content)
    // Refused rather than silently fulfilled, so every way this can decline a
    // draft looks the same from outside: a rejection carrying a reason. A
    // caller that has to know whether the draft left — the composer deciding
    // whether its full-pane editor is finished with — cannot tell "nothing to
    // send" from "sent" otherwise.
    if (!text.trim()) return rejectWithValue({ kind: "empty-draft" })
    if (new TextEncoder().encode(text).length > 8192) {
      dispatch(
        showError({
          id,
          message:
            "This gateway accepts up to 8 KiB of text per message. Your draft has been kept.",
        }),
      )
      return rejectWithValue({ kind: "message-too-large" })
    }
    const serverId = current.serverConversationId ?? crypto.randomUUID()
    const executionId = crypto.randomUUID()
    const actionId = crypto.randomUUID()
    dispatch(bindConversation({ id, serverId }))
    dispatch(
      submissionStarted({
        content: input.content,
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
      const submission = { conversationId: serverId, executionId, actionId, text }
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
          message: detail(error),
          uncertain:
            admissionAttempted &&
            !(error instanceof ConversationUnavailableError) &&
            (!(error instanceof NessaConversationMutationError) || error.uncertain),
        }),
      )
      throw error
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
      dispatch(readFailed({ id, requestId, message: detail(error) }))
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
        const input = {
          conversationId: serverId,
          executionId: control.executionId,
          actionId: turn.actionId,
          text: contentText(turn.content),
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
    if (control.kind === "close") dispatch(cancellationChanged({ id }))
    // A lost acknowledgement may follow an applied control. Read authority again;
    // never replay the control or infer that the previous order still holds.
    await dispatch(refreshConversation(id))
    dispatch(showError({ id, message: detail(error) }))
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
    },
    submissionFailed(
      state,
      action: PayloadAction<{
        id: string
        executionId: string
        message: string
        uncertain?: boolean
      }>,
    ) {
      return failSend(
        state,
        action.payload.id,
        action.payload.executionId,
        action.payload.message,
        action.payload.uncertain,
      )
    },
    showError(state, action: PayloadAction<{ id: string; message: string }>) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current) current.error = action.payload.message
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
      action: PayloadAction<{ id: string; requestId: string; message: string }>,
    ) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current?.readRequest === action.payload.requestId) {
        current.readError = action.payload.message
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
        }
      }
    },
    controlStarted(state, action: PayloadAction<string>) {
      const current = state.conversations.find((item) => item.id === action.payload)
      if (current) {
        current.controlPending = true
        current.readRequest = undefined
        current.error = undefined
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
