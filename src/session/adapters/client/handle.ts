import type { NessaClient } from "@nessa/client"

/** One live transport per application instance, never a process-global service. */
export function createSessionHandle() {
  let client: NessaClient | null = null
  return {
    get: () => client,
    set: (next: NessaClient | null) => {
      client = next
    },
  }
}
