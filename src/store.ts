/**
 * Product state the shell — and later an agent — can dispatch into.
 *
 * Host, DOM, and clocks stay in hooks. This store is the conversation strip.
 */
import { configureStore } from "@reduxjs/toolkit"
import { useDispatch, useSelector } from "react-redux"

import { conversationReducer } from "./conversation-slice"

export function makeStore() {
  return configureStore({
    reducer: {
      conversation: conversationReducer,
    },
  })
}

export const store = makeStore()

export type RootState = ReturnType<typeof store.getState>
export type AppDispatch = typeof store.dispatch

/** For callers that are not inside React: tests, and later the agent. */
export const dispatch: AppDispatch = store.dispatch.bind(store)

export const useAppDispatch = useDispatch.withTypes<AppDispatch>()
export const useAppSelector = useSelector.withTypes<RootState>()

export {
  advanceReply,
  closeConversation,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
} from "./conversation-slice"
