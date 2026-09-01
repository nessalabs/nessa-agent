/**
 * Product state the shell — and later an agent — can dispatch into.
 *
 * The conversation vertical owns its slice. This file is the composition
 * root: it mounts that projection. It imports the slice, not the UI barrel,
 * so tests do not pull the design system.
 */
import { configureStore } from "@reduxjs/toolkit"

import { conversationReducer } from "./conversation/adapters/store/slice"

export function makeStore() {
  return configureStore({
    reducer: {
      conversation: conversationReducer,
    },
  })
}

export const store = makeStore()
