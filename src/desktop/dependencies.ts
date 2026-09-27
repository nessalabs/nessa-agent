/**
 * The desktop window's composition: every outside thing it talks to, built
 * once and handed to the store (`store.ts`) and the views. Overrides are
 * explicit, never a service locator — a test or a future gateway adapter
 * passes its own `workspace`.
 *
 * ```ts
 * const dependencies = createDesktopDependencies({ workspace: gatewaySource })
 * const store = makeDesktopStore(dependencies)
 * ```
 */
import type {
  WorkspaceDependencies,
  WorkspaceSource,
} from "./workspace/application/ports"
import { inMemorySource } from "./workspace/adapters/in-memory/in-memory-source"

export function createDesktopDependencies(
  options: {
    workspace?: WorkspaceSource
    now?: () => number
    newId?: () => string
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
  }
}
