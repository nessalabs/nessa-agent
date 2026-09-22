import { describe, expect, it, vi } from "vitest"
import { createGatewayStartupMonitor } from "./gateway-startup"
import type { GatewayStartup, GatewayStartupSource } from "./ports"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

async function flush() {
  await Promise.resolve()
  await Promise.resolve()
}

describe("observing gateway startup", () => {
  it("does not let a snapshot overwrite a newer subscribed event", async () => {
    const snapshot = deferred<GatewayStartup>()
    let publish!: (startup: GatewayStartup) => void
    const source: GatewayStartupSource = {
      snapshot: () => snapshot.promise,
      subscribe: async (handler) => {
        publish = handler
        return () => undefined
      },
      retry: async () => undefined,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )

    monitor.start()
    await flush()
    publish({ revision: 2, state: "ready" })
    snapshot.resolve({ revision: 1, state: "starting" })
    await flush()

    expect(changed).toEqual([{ revision: 2, state: "ready" }])
  })

  it("does not let a rejected snapshot overwrite a newer subscribed event", async () => {
    const snapshot = deferred<GatewayStartup>()
    let publish!: (startup: GatewayStartup) => void
    const source: GatewayStartupSource = {
      snapshot: () => snapshot.promise,
      subscribe: async (handler) => {
        publish = handler
        return () => undefined
      },
      retry: async () => undefined,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )

    monitor.start()
    await flush()
    publish({ revision: 2, state: "ready" })
    snapshot.reject(new Error("snapshot bridge failed"))
    await flush()

    expect(changed).toEqual([{ revision: 2, state: "ready" }])
  })

  it("keeps the listener after a failed snapshot and recovers on its next event", async () => {
    let publish!: (startup: GatewayStartup) => void
    const stop = vi.fn()
    const source: GatewayStartupSource = {
      snapshot: async () => {
        throw new Error("snapshot bridge failed")
      },
      subscribe: async (handler) => {
        publish = handler
        return stop
      },
      retry: async () => undefined,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )

    monitor.start()
    await flush()
    publish({ revision: 4, state: "ready" })
    monitor.stop()

    expect(changed).toEqual([
      {
        state: "unavailable",
        message: "Nessa could not read its background service startup state.",
      },
      { revision: 4, state: "ready" },
    ])
    expect(stop).toHaveBeenCalledOnce()
  })

  it("removes a subscription that finishes establishing after stop", async () => {
    const subscribed = deferred<() => void>()
    const stop = vi.fn()
    const snapshot = vi.fn<() => Promise<GatewayStartup>>()
    const source: GatewayStartupSource = {
      snapshot,
      subscribe: () => subscribed.promise,
      retry: async () => undefined,
    }
    const changed = vi.fn()
    const monitor = createGatewayStartupMonitor(source, changed)

    monitor.start()
    monitor.stop()
    subscribed.resolve(stop)
    await flush()

    expect(stop).toHaveBeenCalledOnce()
    expect(snapshot).not.toHaveBeenCalled()
    expect(changed).not.toHaveBeenCalled()
  })

  it("redelivers the same snapshot to a restarted lifecycle", async () => {
    const source: GatewayStartupSource = {
      snapshot: async () => ({ revision: 2, state: "ready" }),
      subscribe: async () => () => undefined,
      retry: async () => undefined,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )

    monitor.start()
    await flush()
    monitor.stop()
    monitor.start()
    await flush()

    expect(changed).toEqual([
      { revision: 2, state: "ready" },
      { revision: 2, state: "ready" },
    ])
  })

  it("turns observation and retry transport failures into visible typed states", async () => {
    const source: GatewayStartupSource = {
      snapshot: async () => ({ revision: 0, state: "starting" }),
      subscribe: async () => {
        throw new Error("native event bridge unavailable")
      },
      retry: async () => {
        throw new Error("native command bridge unavailable")
      },
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )

    monitor.start()
    await flush()
    await monitor.retry()

    expect(changed).toEqual([
      {
        state: "unavailable",
        message: "Nessa could not read its background service startup state.",
      },
      {
        state: "unavailable",
        message: "Nessa could not retry its background service startup.",
      },
    ])
  })

  it("does not report a retry rejection after its owner has stopped", async () => {
    const retry = deferred<void>()
    const source: GatewayStartupSource = {
      snapshot: async () => ({ revision: 1, state: "failed", message: "failed" }),
      subscribe: async () => () => undefined,
      retry: () => retry.promise,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )
    monitor.start()
    await flush()
    const attempted = monitor.retry()
    monitor.stop()
    retry.reject(new Error("late rejection"))
    await attempted

    expect(changed).toEqual([{ revision: 1, state: "failed", message: "failed" }])
  })

  it("restores the canonical snapshot at the same revision after a local failure", async () => {
    const retry = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("retry bridge failed"))
      .mockResolvedValueOnce(undefined)
    const source: GatewayStartupSource = {
      snapshot: async () => ({ revision: 3, state: "ready" }),
      subscribe: async () => () => undefined,
      retry,
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )
    monitor.start()
    await flush()

    await monitor.retry()
    await monitor.retry()

    expect(changed).toEqual([
      { revision: 3, state: "ready" },
      {
        state: "unavailable",
        message: "Nessa could not retry its background service startup.",
      },
      { revision: 3, state: "ready" },
    ])
  })

  it("carries an explicit retry from failed through starting to ready", async () => {
    let publish!: (startup: GatewayStartup) => void
    const snapshot = vi
      .fn<() => Promise<GatewayStartup>>()
      .mockResolvedValueOnce({
        revision: 1,
        state: "failed",
        message: "registration failed",
      })
      .mockResolvedValueOnce({ revision: 3, state: "ready" })
    const source: GatewayStartupSource = {
      snapshot,
      subscribe: async (handler) => {
        publish = handler
        return () => undefined
      },
      retry: async () => publish({ revision: 2, state: "starting" }),
    }
    const changed: unknown[] = []
    const monitor = createGatewayStartupMonitor(source, (startup) =>
      changed.push(startup),
    )
    monitor.start()
    await flush()

    await monitor.retry()

    expect(changed).toEqual([
      { revision: 1, state: "failed", message: "registration failed" },
      { revision: 2, state: "starting" },
      { revision: 3, state: "ready" },
    ])
  })
})
