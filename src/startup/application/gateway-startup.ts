import type { GatewayStartup, GatewayStartupSource } from "./ports"

/** What setup can paint from the native lifecycle seam. */
export type GatewayStartupStatus =
  | GatewayStartup
  | {
      readonly state: "unavailable"
      readonly message: string
    }

/** A lifecycle-safe subscription to the host's revisioned startup state. */
export interface GatewayStartupMonitor {
  start(): void
  stop(): void
  retry(): Promise<void>
}

/**
 * Subscribe before taking a snapshot, then retain only the newest revision.
 *
 * An event may arrive while the snapshot command is in flight. The revision is
 * the host's ordering evidence, so the older snapshot cannot move setup back
 * from ready or failed to starting. `start` and `stop` may alternate while a
 * subscription is being established, as React Strict Mode does; an obsolete
 * subscription removes itself when its promise settles.
 */
export function createGatewayStartupMonitor(
  source: GatewayStartupSource,
  changed: (startup: GatewayStartupStatus) => void,
): GatewayStartupMonitor {
  let latestRevision = -1
  let lifetime = 0
  let unlisten: (() => void) | undefined
  let running = false
  let subscribed = false
  let deliveredThisLifetime = false
  let showingHostState = false
  let observations = 0

  function observe(startup: GatewayStartup): void {
    if (startup.revision < latestRevision) return
    if (
      startup.revision === latestRevision &&
      showingHostState &&
      deliveredThisLifetime
    ) {
      return
    }
    latestRevision = startup.revision
    deliveredThisLifetime = true
    showingHostState = true
    observations += 1
    changed(startup)
  }

  function unavailable(message: string): void {
    showingHostState = false
    changed({ state: "unavailable", message })
  }

  async function connect(active: number): Promise<void> {
    const observationsBeforeSnapshot = observations
    let stop: () => void
    try {
      stop = await source.subscribe((startup) => {
        if (active === lifetime) observe(startup)
      })
    } catch {
      if (active === lifetime) {
        unavailable("Nessa could not read its background service startup state.")
      }
      return
    }
    if (active !== lifetime) {
      stop()
      return
    }
    unlisten = stop
    subscribed = true
    try {
      const snapshot = await source.snapshot()
      if (active === lifetime) observe(snapshot)
    } catch {
      // A subscribed event is newer evidence than a failed snapshot from this
      // same attempt. Keep it; the failure must not replace ready.
      if (active === lifetime && observations === observationsBeforeSnapshot) {
        unavailable("Nessa could not read its background service startup state.")
      }
    }
  }

  function begin(): void {
    const active = (lifetime += 1)
    running = true
    subscribed = false
    deliveredThisLifetime = false
    void connect(active)
  }

  return {
    start() {
      if (!running) begin()
    },
    stop() {
      lifetime += 1
      running = false
      subscribed = false
      unlisten?.()
      unlisten = undefined
    },
    async retry() {
      const active = lifetime
      const observationsBeforeRetry = observations
      try {
        await source.retry()
        if (!running || active !== lifetime) return
        if (!subscribed) {
          begin()
          return
        }
        const snapshot = await source.snapshot()
        if (running && active === lifetime) observe(snapshot)
      } catch {
        if (running && active === lifetime && observations === observationsBeforeRetry) {
          unavailable("Nessa could not retry its background service startup.")
        }
      }
    },
  }
}
