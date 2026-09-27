/**
 * The desktop window's product state, which the window — and an agent — can
 * dispatch into. It mounts the workspace vertical's slice and hands the
 * workspace's commands their dependencies as the thunk extra argument; the
 * same dependencies run the workspace's effects. Tests make a store with a
 * source of their own.
 *
 * ```ts
 * const store = makeDesktopStore(createDesktopDependencies())
 * store.dispatch(openBeside({ sessionId: "retry" }))
 * ```
 */
import { configureStore } from "@reduxjs/toolkit"
import type { WorkspaceDependencies } from "./workspace/application/ports"
import { workspaceEffects } from "./workspace/adapters/store/effects"
import { workspaceReducer } from "./workspace/adapters/store/slice"

export function makeDesktopStore(dependencies: WorkspaceDependencies) {
  const effects = workspaceEffects(dependencies)
  return configureStore({
    reducer: { workspace: workspaceReducer },
    middleware: (getDefaultMiddleware) =>
      getDefaultMiddleware({ thunk: { extraArgument: dependencies } }).prepend(
        effects.middleware,
      ),
  })
}

export type DesktopStore = ReturnType<typeof makeDesktopStore>
export type DesktopState = ReturnType<DesktopStore["getState"]>
export type DesktopDispatch = DesktopStore["dispatch"]
