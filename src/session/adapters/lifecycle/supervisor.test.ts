import { afterEach, describe, expect, it, vi } from "vitest"
import { NessaConnectionClosedError } from "@nessa/client"
import { createSessionHandle } from "../client/handle"
import { SessionHealthError, type EstablishedDevSession } from "../client/dev-session"
import { superviseSession } from "./supervisor"

type Client = EstablishedDevSession["client"]
type State = Client["connectionState"]
const interrupted = () => new NessaConnectionClosedError(1006, "transport interrupted")
function connection(initial?: Error) {
  let state: State = initial
    ? { status: "closed", error: initial }
    : { status: "connected" }
  const closers = new Set<(error: Error) => void>()
  const observers = new Set<(state: State) => void>()
  const close = vi.fn()
  const client = {
    get connectionState() {
      return state
    },
    productSession: { gatewayId: "gateway" },
    close,
    onClose(callback: (error: Error) => void) {
      if (initial) callback(initial)
      closers.add(callback)
      return () => {
        closers.delete(callback)
      }
    },
    onConnectionStateChange(callback: (state: State) => void) {
      observers.add(callback)
      return () => {
        observers.delete(callback)
      }
    },
  } as unknown as Client
  return {
    established: {
      client,
      hello: client.productSession,
      health: { ok: true, runtimeStatus: "ready", uptimeMs: 1 },
    } as EstablishedDevSession,
    close,
    closeCallbacks: () => [...closers],
    disconnect(error: Error) {
      state = { status: "closed", error }
      for (const callback of [...closers]) callback(error)
    },
    state(next: State) {
      state = next
      for (const callback of observers) callback(next)
    },
  }
}
function setup(connect: () => Promise<EstablishedDevSession>) {
  const session = createSessionHandle()
  const connecting = vi.fn(),
    reconnecting = vi.fn(),
    ready = vi.fn(),
    failed = vi.fn()
  const dispose = superviseSession({
    session,
    connect,
    connecting,
    reconnecting,
    ready,
    failed,
  })
  return { session, connecting, reconnecting, ready, failed, dispose }
}

afterEach(() => vi.useRealTimers())
describe("application session supervision", () => {
  it("retries exhausted startup transport attempts until the gateway returns", async () => {
    vi.useFakeTimers()
    const active = connection()
    const connect = vi
      .fn()
      .mockRejectedValueOnce(interrupted())
      .mockRejectedValueOnce(interrupted())
      .mockResolvedValue(active.established)
    const owner = setup(connect)
    await vi.advanceTimersByTimeAsync(1_500)
    expect(connect).toHaveBeenCalledTimes(3)
    expect(owner.session.get()).toBe(active.established.client)
    expect(owner.failed).not.toHaveBeenCalled()
    owner.dispose()
  })
  it("starts a fresh client after exhausted live retries and ignores stale callbacks", async () => {
    vi.useFakeTimers()
    const first = connection(),
      second = connection()
    const connect = vi
      .fn()
      .mockResolvedValueOnce(first.established)
      .mockResolvedValue(second.established)
    const owner = setup(connect)
    await vi.advanceTimersByTimeAsync(0)
    const stale = first.closeCallbacks()[0]
    first.disconnect(interrupted())
    expect(owner.session.get()).toBeNull()
    await vi.advanceTimersByTimeAsync(500)
    stale(interrupted())
    await vi.advanceTimersByTimeAsync(10_000)
    expect(connect).toHaveBeenCalledTimes(2)
    expect(owner.session.get()).toBe(second.established.client)
    owner.dispose()
  })
  it("lets the current client reconnect without starting a competing connection", async () => {
    vi.useFakeTimers()
    const active = connection(),
      connect = vi.fn().mockResolvedValue(active.established)
    const owner = setup(connect)
    await vi.advanceTimersByTimeAsync(0)
    active.state({ status: "reconnecting", attempt: 1 } as State)
    expect(owner.session.get()).toBeNull()
    expect(owner.reconnecting).toHaveBeenCalledOnce()
    expect(owner.connecting).toHaveBeenCalledOnce()
    expect(owner.ready).toHaveBeenCalledOnce()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(connect).toHaveBeenCalledTimes(1)
    active.state({ status: "connected" })
    expect(owner.session.get()).toBe(active.established.client)
    expect(owner.reconnecting).toHaveBeenCalledOnce()
    expect(owner.ready).toHaveBeenCalledTimes(2)
    owner.dispose()
  })
  it("does not automatically retry terminal authentication errors", async () => {
    vi.useFakeTimers()
    const connect = vi.fn().mockRejectedValue(new Error("Credential unavailable"))
    const owner = setup(connect)
    await vi.advanceTimersByTimeAsync(60_000)
    expect(connect).toHaveBeenCalledTimes(1)
    expect(owner.failed).toHaveBeenCalledWith("Credential unavailable")
    owner.dispose()
    // An explicit Retry remounts the lifetime, with fresh credential discovery.
    const next = connection(),
      retry = setup(vi.fn().mockResolvedValue(next.established))
    await vi.advanceTimersByTimeAsync(0)
    expect(retry.session.get()).toBe(next.established.client)
    retry.dispose()
  })
  it("cancels pending retry timers when the application unmounts", async () => {
    vi.useFakeTimers()
    const connect = vi.fn().mockRejectedValue(interrupted()),
      owner = setup(connect)
    await vi.advanceTimersByTimeAsync(0)
    owner.dispose()
    await vi.advanceTimersByTimeAsync(60_000)
    expect(connect).toHaveBeenCalledTimes(1)
  })
  it("closes a late connection after owner disposal without publishing readiness", async () => {
    let resolve!: (value: EstablishedDevSession) => void
    const owner = setup(
      () =>
        new Promise((done) => {
          resolve = done
        }),
    )
    owner.dispose()
    const late = connection()
    resolve(late.established)
    await Promise.resolve()
    expect(late.close).toHaveBeenCalledOnce()
    expect(owner.ready).not.toHaveBeenCalled()
    expect(owner.session.get()).toBeNull()
  })
  it("recovers an already-closed returned client without publishing stale readiness", async () => {
    vi.useFakeTimers()
    const closed = connection(interrupted()),
      active = connection()
    const owner = setup(
      vi
        .fn()
        .mockResolvedValueOnce(closed.established)
        .mockResolvedValue(active.established),
    )
    await vi.advanceTimersByTimeAsync(0)
    expect(owner.ready).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(500)
    expect(owner.session.get()).toBe(active.established.client)
    owner.dispose()
  })
  it("uses the typed health-check cause to recover a transport interruption", async () => {
    vi.useFakeTimers()
    const connect = vi
      .fn()
      .mockRejectedValueOnce(new SessionHealthError("health interrupted", interrupted()))
      .mockResolvedValue(connection().established)
    const owner = setup(connect)
    await vi.advanceTimersByTimeAsync(500)
    expect(owner.ready).toHaveBeenCalledOnce()
    owner.dispose()
  })
})

it("reports terminal failures to the owning browser lifecycle after detaching the client", async () => {
  vi.useFakeTimers()
  const active = connection()
  const session = createSessionHandle()
  const onTerminalFailure = vi.fn(() => expect(session.get()).toBeNull())
  const dispose = superviseSession({
    session,
    connect: async () => active.established,
    connecting: vi.fn(),
    reconnecting: vi.fn(),
    ready: vi.fn(),
    failed: vi.fn(),
    onTerminalFailure,
  })
  await vi.advanceTimersByTimeAsync(0)
  const expired = new NessaConnectionClosedError(4003, "")
  active.disconnect(expired)
  expect(onTerminalFailure).toHaveBeenCalledWith(expired)
  dispose()
})
