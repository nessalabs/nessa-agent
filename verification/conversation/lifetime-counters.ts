/** Follow, stop and close counts for the local-tab lifetime fixture. */

export interface LifetimeSnapshot {
  readonly follows: readonly string[]
  readonly stops: number
  readonly closed: readonly string[]
}

export interface LifetimeCounters {
  noteFollow(id: string): void
  noteStop(): void
  noteClose(id: string): void
  snapshot(): LifetimeSnapshot
}

export interface LifetimeOwner {
  snapshot(): LifetimeSnapshot | null
}

declare global {
  interface Window {
    /** Live counters. A render is not required to read them. */
    __localTabLifetimes?: LifetimeOwner
  }
}

/** The fixture's counters. Cleanup updates these; nothing here renders. */
export function createLifetimeCounters(): LifetimeCounters {
  const follows: string[] = []
  const closed: string[] = []
  let stops = 0
  return {
    noteFollow(id) {
      follows.push(id)
    },
    noteStop() {
      stops += 1
    },
    noteClose(id) {
      closed.push(id)
    },
    snapshot() {
      return { follows: follows.slice(), stops, closed: closed.slice() }
    },
  }
}

/** Publish `counters` for the browser verifier. Replaces any earlier owner. */
export function publishLifetimeCounters(counters: LifetimeCounters): void {
  window.__localTabLifetimes = { snapshot: () => counters.snapshot() }
}
