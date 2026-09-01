import { createSlice, type PayloadAction } from "@reduxjs/toolkit"

import { emptyLocalStrip } from "../../application/local-strip"
import { localConversationGateway as gateway } from "../gateway/local"

const conversationSlice = createSlice({
  name: "conversation",
  initialState: emptyLocalStrip(),
  reducers: {
    setActiveId(state, action: PayloadAction<string>) {
      return gateway.setActive(state, action.payload)
    },
    setDraft(state, action: PayloadAction<{ draft: string; id?: string }>) {
      return gateway.setDraft(state, action.payload)
    },
    sendDraft(state, action: PayloadAction<{ id?: string } | undefined>) {
      return gateway.sendDraft(state, action.payload?.id)
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
    advanceReply(state, action: PayloadAction<{ conversationId: string }>) {
      return gateway.advanceReply(state, action.payload.conversationId)
    },
  },
})

export const {
  setActiveId,
  setDraft,
  sendDraft,
  openConversation,
  closeConversation,
  stopGenerating,
  advanceReply,
} = conversationSlice.actions

export const conversationReducer = conversationSlice.reducer
