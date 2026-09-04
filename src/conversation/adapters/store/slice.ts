import { createAsyncThunk, createSlice, type PayloadAction } from "@reduxjs/toolkit"

import { emptyLocalTabs } from "../../application/local-tabs"
import {
  beginSend,
  completeEcho,
  failSend,
} from "../../application/usecases/send-draft"
import { getSessionClient } from "../../../session"
import { localConversationGateway as gateway } from "../gateway/local"

export type SendDraftArg = {
  text: string
  id?: string
}

export const sendDraft = createAsyncThunk(
  "conversation/sendDraft",
  async (input: SendDraftArg, { rejectWithValue }) => {
    const text = input.text.trim()
    if (!text) {
      return rejectWithValue("empty draft")
    }

    const client = getSessionClient()
    if (!client) {
      return rejectWithValue("not connected")
    }

    const result = await client.conversation.echo(text)
    return {
      text: result.text,
    }
  },
)

const conversationSlice = createSlice({
  name: "conversation",
  initialState: emptyLocalTabs(),
  reducers: {
    setActive(state, action: PayloadAction<string>) {
      return gateway.setActive(state, action.payload)
    },
    setDraft(state, action: PayloadAction<{ draft: string; id?: string }>) {
      return gateway.setDraft(state, action.payload)
    },
    openConversation(state) {
      return gateway.openConversation(state)
    },
    closeConversation(state, action: PayloadAction<string>) {
      return gateway.closeConversation(state, action.payload)
    },
    stopGenerating(
      state,
      action: PayloadAction<{ conversationId?: string } | undefined>,
    ) {
      return gateway.stopGenerating(state, action.payload?.conversationId)
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(sendDraft.pending, (state, action) => {
        return beginSend(state, {
          text: action.meta.arg.text,
          conversationId: action.meta.arg.id,
        })
      })
      .addCase(sendDraft.fulfilled, (state, action) => {
        const id = action.meta.arg.id ?? state.activeId
        return completeEcho(state, id, action.payload.text)
      })
      .addCase(sendDraft.rejected, (state, action) => {
        if (action.payload === "empty draft") return state
        const id = action.meta.arg.id ?? state.activeId
        return failSend(state, id)
      })
  },
})

export const {
  setActive,
  setDraft,
  openConversation,
  closeConversation,
  stopGenerating,
} = conversationSlice.actions

export const conversationReducer = conversationSlice.reducer
