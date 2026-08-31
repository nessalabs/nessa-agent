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
  type Conversation,
} from "./conversation"

export type ConversationStrip = {
  conversations: Conversation[]
  activeId: string
  nextId: number
}

const initialState: ConversationStrip = {
  conversations: [conversation("c0")],
  activeId: "c0",
  nextId: 1,
}

function takeId(state: ConversationStrip) {
  const id = state.nextId
  state.nextId += 1
  return id
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
      if (current) current.draft = action.payload.draft
    },
    sendDraft(state, action: PayloadAction<{ id?: string } | undefined>) {
      const id = action.payload?.id ?? state.activeId
      const current = state.conversations.find((item) => item.id === id)
      if (!current || current.draft.trim() === "" || current.phase !== "idle") {
        return
      }
      replace(state, send(current, current.draft, `t${takeId(state)}`))
    },
    openConversation(state) {
      const id = `c${takeId(state)}`
      state.conversations.push(conversation(id))
      state.activeId = id
    },
    closeConversation(state, action: PayloadAction<string>) {
      const next = closeInStrip(
        state.conversations,
        action.payload,
        state.activeId,
        () => `c${takeId(state)}`,
      )
      state.conversations = next.items
      state.activeId = next.activeId
    },
    stopGenerating(state, action: PayloadAction<{ id?: string } | undefined>) {
      const id = action.payload?.id ?? state.activeId
      const current = state.conversations.find((item) => item.id === id)
      if (current) replace(state, stop(current))
    },
    /** The stand-in clock calls this; a real runtime will dispatch it too. */
    advanceReply(state, action: PayloadAction<{ id: string }>) {
      const current = state.conversations.find((item) => item.id === action.payload.id)
      if (current) replace(state, advance(current, `t${takeId(state)}`))
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
