/**
 * Product state the shell — and later an agent — can dispatch into.
 *
 * Verticals own their slices. This file is the composition root: it mounts
 * those projections. It imports slices, not UI barrels, so tests do not pull
 * the design system.
 */
import { configureStore } from "@reduxjs/toolkit"

import { conversationReducer } from "./conversation/adapters/store/slice"
import { sessionReducer } from "./session/adapters/store/slice"

export function makeStore() {
  return configureStore({
    reducer: {
      conversation: conversationReducer,
      session: sessionReducer,
    },
  })
}

export const store = makeStore()

export type AppStore = ReturnType<typeof makeStore>
export type RootState = ReturnType<AppStore["getState"]>
export type AppDispatch = AppStore["dispatch"]
