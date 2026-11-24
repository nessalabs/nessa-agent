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
import {
  carryOutWindowChange,
  initialWorkspaceFrom,
  terminalsById,
  terminalsListed,
  workspaceActions,
  workspaceEffects,
  workspaceReducer,
  type WorkspaceDependencies,
} from "./workspace"

export function makeDesktopStore(dependencies: WorkspaceDependencies) {
  const effects = workspaceEffects(dependencies)
  const store = configureStore({
    reducer: { workspace: workspaceReducer },
    // What the window kept between launches, read once.
    preloadedState: { workspace: initialWorkspaceFrom(dependencies) },
    middleware: (getDefaultMiddleware) =>
      getDefaultMiddleware({ thunk: { extraArgument: dependencies } }).prepend(
        effects.middleware,
      ),
  })
  // The changes the person's agents, or their CLI, ask of this window are
  // carried out as a click here would, for as long as the window runs.
  dependencies.windowCommands?.listen((change) =>
    carryOutWindowChange(store, change, after),
  )
  // The person's terminals, as the gateway lists them, for as long as the window runs.
  dependencies.terminals?.follow((terminals) =>
    store.dispatch(terminalsListed({ terminals: terminalsById(terminals) })),
  )
  // Multi-gateway registry snapshots, for sidebar status badges and Settings › Gateways.
  dependencies.gatewayRegistry?.subscribe((gateways) =>
    store.dispatch(workspaceActions.gatewaysUpdated({ gateways })),
  )
  return store
}

/** The page's own timer, for how long a change waits on the layout. */
function after(ms: number, run: () => void): () => void {
  const timer = setTimeout(run, ms)
  return () => clearTimeout(timer)
}

export type DesktopStore = ReturnType<typeof makeDesktopStore>
export type DesktopState = ReturnType<DesktopStore["getState"]>
export type DesktopDispatch = DesktopStore["dispatch"]
