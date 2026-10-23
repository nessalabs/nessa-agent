import { describe, expect, it, vi } from "vitest"
import { createGatewayStartupMonitor } from "./gateway-startup"
import type { GatewayStartup, GatewayStartupSource } from "./ports"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}
const flush = async () => {
  for (let i = 0; i < 6; i++) await Promise.resolve()
}
function fixture() {
  const listeners: Array<(startup: GatewayStartup) => void> = []
  const subscriptions: Array<ReturnType<typeof deferred<() => void>>> = []
  const snapshots: Array<ReturnType<typeof deferred<GatewayStartup>>> = []
  const retries: Array<ReturnType<typeof deferred<void>>> = []
  const source: GatewayStartupSource = {
    subscribe: vi.fn((listener) => {
      listeners.push(listener)
      const pending = deferred<() => void>()
      subscriptions.push(pending)
      return pending.promise
    }),
    snapshot: vi.fn(() => {
      const pending = deferred<GatewayStartup>()
      snapshots.push(pending)
      return pending.promise
    }),
    retry: vi.fn(() => {
      const pending = deferred<void>()
      retries.push(pending)
      return pending.promise
    }),
  }
  const changed = vi.fn()
  const monitor = createGatewayStartupMonitor(source, changed)
  return { source, monitor, changed, listeners, subscriptions, snapshots, retries }
}
const hostStates: GatewayStartup[] = [
  { revision: 7, state: "unmanaged" },
  { revision: 7, state: "starting", step: "launching" },
  { revision: 7, state: "failed", message: "service refused" },
  { revision: 7, state: "ready" },
]

describe("gateway startup observation ordering", () => {
  it.each(hostStates)(
    "retains $state against duplicate and older events and snapshots",
    async (event) => {
      const f = fixture()
      f.monitor.start()
      // Native events may run before subscribe's promise returns its disposer.
      f.listeners[0]!(event)
      f.listeners[0]!({ revision: 6, state: "starting", step: "preparing" })
      f.listeners[0]!(event)
      const unlisten = vi.fn()
      f.subscriptions[0]!.resolve(unlisten)
      await flush()
      f.snapshots[0]!.resolve({ revision: 6, state: "ready" })
      await flush()
      expect(f.changed.mock.calls).toEqual([[event]])
      f.listeners[0]!({ revision: 8, state: "ready" })
      expect(f.changed).toHaveBeenLastCalledWith({ revision: 8, state: "ready" })
      f.monitor.stop()
      expect(unlisten).toHaveBeenCalledOnce()
    },
  )

  it.each(["resolved", "rejected"] as const)(
    "ignores a %s snapshot from a retired lifetime",
    async (outcome) => {
      const f = fixture()
      const oldStop = vi.fn(),
        nextStop = vi.fn()
      f.monitor.start()
      f.subscriptions[0]!.resolve(oldStop)
      await flush()
      f.monitor.stop()
      f.monitor.start()
      f.subscriptions[1]!.resolve(nextStop)
      await flush()
      f.snapshots[1]!.resolve({ revision: 4, state: "ready" })
      await flush()
      if (outcome === "resolved")
        f.snapshots[0]!.resolve({ revision: 99, state: "failed", message: "old" })
      else f.snapshots[0]!.reject(new Error("old bridge"))
      f.listeners[0]!({ revision: 100, state: "failed", message: "old event" })
      await flush()
      expect(f.changed.mock.calls).toEqual([[{ revision: 4, state: "ready" }]])
      expect(oldStop).toHaveBeenCalledOnce()
      f.monitor.stop()
      expect(nextStop).toHaveBeenCalledOnce()
    },
  )

  it("makes repeated start and stop calls retain one subscription owner", async () => {
    const f = fixture()
    const unlisten = vi.fn()
    f.monitor.start()
    f.monitor.start()
    expect(f.source.subscribe).toHaveBeenCalledOnce()
    f.subscriptions[0]!.resolve(unlisten)
    await flush()
    f.snapshots[0]!.resolve({ revision: 1, state: "ready" })
    await flush()
    f.monitor.stop()
    f.monitor.stop()
    expect(unlisten).toHaveBeenCalledOnce()
    f.listeners[0]!({ revision: 2, state: "failed", message: "stale" })
    expect(f.changed).toHaveBeenCalledOnce()
  })

  it("removes a pending Strict Mode subscription while preserving the replacement", async () => {
    const f = fixture()
    const oldStop = vi.fn(),
      nextStop = vi.fn()
    f.monitor.start()
    f.monitor.stop()
    f.monitor.start()
    f.subscriptions[1]!.resolve(nextStop)
    await flush()
    f.snapshots[0]!.resolve({ revision: 2, state: "ready" })
    f.subscriptions[0]!.resolve(oldStop)
    await flush()
    expect(f.source.snapshot).toHaveBeenCalledOnce()
    expect(oldStop).toHaveBeenCalledOnce()
    expect(nextStop).not.toHaveBeenCalled()
    expect(f.changed.mock.calls).toEqual([[{ revision: 2, state: "ready" }]])
    f.monitor.stop()
    expect(nextStop).toHaveBeenCalledOnce()
  })

  it.each(["resolved", "rejected"] as const)(
    "does not let a %s retry from an old lifetime affect its replacement",
    async (outcome) => {
      const f = fixture()
      f.monitor.start()
      f.subscriptions[0]!.resolve(vi.fn())
      await flush()
      f.snapshots[0]!.resolve({ revision: 1, state: "failed", message: "failed" })
      await flush()
      const retried = f.monitor.retry()
      f.monitor.stop()
      f.monitor.start()
      f.subscriptions[1]!.resolve(vi.fn())
      await flush()
      f.snapshots[1]!.resolve({ revision: 2, state: "ready" })
      await flush()
      if (outcome === "resolved") f.retries[0]!.resolve()
      else f.retries[0]!.reject(new Error("old retry failed"))
      await retried
      expect(f.source.snapshot).toHaveBeenCalledTimes(2)
      expect(f.source.subscribe).toHaveBeenCalledTimes(2)
      expect(f.changed.mock.calls).toEqual([
        [{ revision: 1, state: "failed", message: "failed" }],
        [{ revision: 2, state: "ready" }],
      ])
      f.monitor.stop()
    },
  )

  it("keeps a newer event when a retry command rejects", async () => {
    const f = fixture()
    f.monitor.start()
    f.subscriptions[0]!.resolve(vi.fn())
    await flush()
    f.snapshots[0]!.resolve({ revision: 1, state: "failed", message: "failed" })
    await flush()
    const retried = f.monitor.retry()
    f.listeners[0]!({ revision: 2, state: "ready" })
    f.retries[0]!.reject(new Error("lost acknowledgement"))
    await retried
    expect(f.changed).toHaveBeenCalledTimes(2)
    expect(f.changed).toHaveBeenLastCalledWith({ revision: 2, state: "ready" })
    f.monitor.stop()
  })

  it("resubscribes after an observation failure when retry succeeds", async () => {
    const f = fixture()
    f.monitor.start()
    f.subscriptions[0]!.reject(new Error("event bridge failed"))
    await flush()
    expect(f.changed).toHaveBeenCalledWith(
      expect.objectContaining({ state: "unavailable" }),
    )
    const retried = f.monitor.retry()
    f.retries[0]!.resolve()
    await retried
    expect(f.source.subscribe).toHaveBeenCalledTimes(2)
    const unlisten = vi.fn()
    f.subscriptions[1]!.resolve(unlisten)
    await flush()
    f.snapshots[0]!.resolve({ revision: 3, state: "ready" })
    await flush()
    expect(f.changed).toHaveBeenLastCalledWith({ revision: 3, state: "ready" })
    f.monitor.stop()
    expect(unlisten).toHaveBeenCalledOnce()
  })
})
