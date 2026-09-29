/**
 * The desktop window's composition: every outside thing it talks to — the
 * workspace source, the clock, ids, the page's measure of the panes' room,
 * the storage that keeps the overview's filter —
 * built once and handed to the store (`store.ts`) and the views. Overrides are
 * explicit, never a service locator — a test or a future gateway adapter
 * passes its own `workspace`.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({ workspace: gatewaySource })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import {
  inMemorySource,
  measureWorkspace,
  rememberedFilter,
  type WorkspaceDependencies,
  type WorkspaceSource,
} from "./workspace"

export function createDesktopDependencies(
  options: {
    workspace?: WorkspaceSource
    now?: () => number
    newId?: () => string
    measure?: WorkspaceDependencies["measure"]
    overviewFilter?: WorkspaceDependencies["overviewFilter"]
  } = {},
): WorkspaceDependencies {
  // The real clock and timers; tests pass their own.
  const now = options.now ?? (() => Date.now())
  const workspace =
    options.workspace ??
    inMemorySource({
      now,
      after: (ms, run) => {
        const timer = window.setTimeout(run, ms)
        return () => window.clearTimeout(timer)
      },
    })
  return {
    workspace,
    now,
    newId: options.newId ?? (() => crypto.randomUUID()),
    // The page's own layout: every command that changes the panes is held to it.
    measure: options.measure ?? (() => measureWorkspace(document)),
    // The webview's storage, where the overview's filter is kept between launches.
    overviewFilter: options.overviewFilter ?? rememberedFilter(),
  }
}
