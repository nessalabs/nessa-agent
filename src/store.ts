/**
 * Product state the shell — and later an agent — can dispatch into.
 *
 * Verticals own their slices. This file is the composition root: it mounts
 * those projections. It imports slices, not UI barrels, so tests do not pull
 * the design system.
 */
import { configureStore } from "@reduxjs/toolkit"

import {
  conversationHistoryReducer,
  attachmentResourceUsage,
  conversationReducer,
} from "./conversation/adapters/store/slice"
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
      conversationHistory: conversationHistoryReducer,
      session: sessionReducer,
    },
  })
  // One store lifetime owns its retained resources. The application publishes
  // whether shared originals are still needed or retained only for previews;
  // every command reconciles, including rejected attachment admissions.
  store.subscribe(() => {
    dependencies.attachments.retain(
      attachmentResourceUsage(store.getState().conversation),
    )
  })
  return store
}

export type AppStore = ReturnType<typeof makeStore>
export type RootState = ReturnType<AppStore["getState"]>
export type AppDispatch = AppStore["dispatch"]
