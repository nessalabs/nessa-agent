/**
 * The conversation strip as a Redux slice.
 *
 * Rules stay in `conversation.ts`. This file is the adapter: named actions
 * an agent (or a test) can dispatch, wrapping those rules. Timers, the host,
 * and the DOM do not live here.
 */
import { createSlice, type PayloadAction } from "@reduxjs/toolkit"

import {
  advance,
  closeInStrip,
  conversation,
  send,
  stop,
  withDraft,
  type Conversation,
} from "./conversation"

export type ConversationStrip = {
  conversations: Conversation[]
  activeId: string
  nextConversationId: number
  nextTurnId: number
}

const initialState: ConversationStrip = {
  conversations: [conversation("c0")],
  activeId: "c0",
  nextConversationId: 1,
  nextTurnId: 1,
}

function takeConversationId(state: ConversationStrip) {
  const id = state.nextConversationId
  state.nextConversationId += 1
  return `c${id}`
}

function takeTurnId(state: ConversationStrip) {
  const id = state.nextTurnId
  state.nextTurnId += 1
  return `t${id}`
}

function replace(state: ConversationStrip, next: Conversation) {
  const index = state.conversations.findIndex((item) => item.id === next.id)
  if (index !== -1) state.conversations[index] = next
}

const conversationSlice = createSlice({
  name: "conversation",
  initialState,
  reducers: {
    setActiveId(state, action: PayloadAction<string>) {
      if (state.conversations.some((item) => item.id === action.payload)) {
        state.activeId = action.payload
      }
    },
    setDraft(state, action: PayloadAction<{ draft: string; id?: string }>) {
      const id = action.payload.id ?? state.activeId
      const current = state.conversations.find((item) => item.id === id)
      if (current) replace(state, withDraft(current, action.payload.draft))
    },
    sendDraft(state, action: PayloadAction<{ id?: string } | undefined>) {
      const id = action.payload?.id ?? state.activeId
      const current = state.conversations.find((item) => item.id === id)
      if (!current) return
      const next = send(current, current.draft, takeTurnId(state), takeTurnId(state))
      if (next === current) return
      replace(state, next)
    },
    openConversation(state) {
      const id = takeConversationId(state)
      state.conversations.push(conversation(id))
      state.activeId = id
    },
    closeConversation(state, action: PayloadAction<string>) {
      const next = closeInStrip(state.conversations, action.payload, state.activeId, () =>
        takeConversationId(state),
      )
      state.conversations = next.items
      state.activeId = next.activeId
    },
    stopGenerating(
      state,
      action: PayloadAction<{ conversationId?: string } | undefined>,
    ) {
      const id = action.payload?.conversationId ?? state.activeId
      const current = state.conversations.find((item) => item.id === id)
      if (current) replace(state, stop(current))
    },
    /** The stand-in clock calls this. A real runtime will replace it. */
    advanceReply(state, action: PayloadAction<{ conversationId: string }>) {
      const current = state.conversations.find(
        (item) => item.id === action.payload.conversationId,
      )
      if (current) replace(state, advance(current, takeTurnId(state)))
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
