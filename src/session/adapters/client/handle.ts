import type { NessaClient } from "@nessa/client"

/** One live transport per application instance, never a process-global service. */
export function createSessionHandle() {
  const listeners = new Set<() => void>()
  let client: NessaClient | null = null
  return {
    get: () => client,
    subscribe: (listener: () => void) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    set: (next: NessaClient | null) => {
      if (client === next) return
      client = next
      for (const listener of listeners) listener()
    },
  }
}
