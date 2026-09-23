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

/** The dependencies the store itself owns and passes to conversation commands. */
export type StoreDependencies = Pick<
  AppDependencies,
  "attachments" | "canChoosePaths" | "conversation"
>

export function makeStore(dependencies: StoreDependencies = createDependencies()) {
  const store = configureStore({
    middleware: (getDefaultMiddleware) =>
      getDefaultMiddleware({
        thunk: {
          extraArgument: {
            conversation: dependencies.conversation,
            canChoosePaths: dependencies.canChoosePaths,
          },
        },
      }),
    reducer: {
      conversation: conversationReducer,
      session: sessionReducer,
    },
  })
  // The store lifetime owns resources; React remounts must not invalidate previews.
  // Every command reconciles, including rejected attachment admissions.
  //
  // A sent turn keeps its images' previews while the turn is on screen: the
  // transcript paints them from the same object URLs, and a send the gateway
  // refuses puts the turn's files back into the draft, where an already-revoked
  // URL would be a broken tile.
  //
  // Once the gateway has a message, its files can never return to a draft, so
  // they stop counting against what may be attached; the conversation slice
  // bounds how many of those originals it keeps at all.
  store.subscribe(() => {
    const ids = new Set<string>()
    const sent = new Set<string>()
    for (const conversation of store.getState().conversation.conversations) {
      for (const part of conversation.draft) if (part.type === "file") ids.add(part.id)
      for (const turn of conversation.turns) {
        if (turn.from !== "user") continue
        const taken = ["accepted", "queued", "delivered"].includes(turn.receipt)
        for (const part of turn.content) {
          if (part.type !== "file") continue
          ids.add(part.id)
          if (taken) sent.add(part.id)
        }
      }
    }
    dependencies.attachments.retain(ids, sent)
  })
  return store
}

export type AppStore = ReturnType<typeof makeStore>
export type RootState = ReturnType<AppStore["getState"]>
export type AppDispatch = AppStore["dispatch"]
