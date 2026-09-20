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

import { createDependencies, type AppDependencies } from "./composition/dependencies"

export function makeStore(dependencies: AppDependencies = createDependencies()) {
  const store = configureStore({
    middleware: (getDefaultMiddleware) =>
      getDefaultMiddleware({
        thunk: { extraArgument: { conversation: dependencies.conversation } },
      }),
    reducer: {
      conversation: conversationReducer,
      session: sessionReducer,
    },
  })
  // The store lifetime owns resources; React remounts must not invalidate previews.
  // Every command reconciles, including rejected attachment admissions.
  //
  // A sent turn keeps its images' previews for as long as the turn is on
  // screen: the transcript paints them from the same object URLs, and a send
  // the gateway refuses puts the turn's files back into the draft, where an
  // already-revoked URL would be a broken tile. They count against the same
  // session budget as drafts, and closing the conversation releases them.
  store.subscribe(() => {
    const ids = new Set(
      store
        .getState()
        .conversation.conversations.flatMap((conversation) =>
          [
            ...conversation.draft,
            ...conversation.turns.flatMap((turn) =>
              turn.from === "user" ? turn.content : [],
            ),
          ].flatMap((part) => (part.type === "file" ? [part.id] : [])),
        ),
    )
    dependencies.attachments.retain(ids)
  })
  return store
}

export type AppStore = ReturnType<typeof makeStore>
export type RootState = ReturnType<AppStore["getState"]>
export type AppDispatch = AppStore["dispatch"]
