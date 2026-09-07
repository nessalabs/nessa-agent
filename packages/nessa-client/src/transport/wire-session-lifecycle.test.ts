import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { WireSession } from "./wire-session.js"

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal("WebSocket", { OPEN: 1 })
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

function fixture() {
  const close = vi.fn()
  const session = new WireSession({
    readyState: 1,
    send() {},
    addEventListener() {},
    close,
  } as unknown as WebSocket)
  return { session, close }
}

it.each(["client", "transport"] as const)(
  "%s closure cleans up a busy session without affecting another session",
  async (closure) => {
    const { session } = fixture()
    const other = fixture()
    const otherClosed = vi.fn()
    other.session.onClose(otherClosed)
    const otherPending = other.session.request("server.health", {})
    const result = Promise.allSettled(
      Array.from({ length: 1000 }, () => session.request("server.health", {})),
    )
    expect(vi.getTimerCount()).toBe(1001)
    const notified = vi.fn()
    session.onClose(notified)
    const closeCode = closure === "client" ? 1000 : 1006
    if (closure === "client") session.close()
    else session.dispatchClose(1006, "transport disconnected")
    session.dispatchClose(1006, "late transport close")
    for (const value of await result) {
      expect(value.status).toBe("rejected")
      if (value.status === "rejected")
        expect(value.reason).toMatchObject({ code: closeCode })
    }
    expect(notified).toHaveBeenCalledTimes(1)
    expect(vi.getTimerCount()).toBe(1)
    expect(otherClosed).not.toHaveBeenCalled()
    expect(other.close).not.toHaveBeenCalled()
    other.session.dispatchFrame({ type: "res", id: "1", ok: true, payload: { ok: true } })
    await expect(otherPending).resolves.toEqual({ ok: true })
    const next = other.session.request("server.health", {})
    other.session.dispatchFrame({ type: "res", id: "2", ok: true, payload: { ok: true } })
    await expect(next).resolves.toEqual({ ok: true })
    expect(vi.getTimerCount()).toBe(0)
    other.session.close()
    await expect(session.request("server.health", {})).rejects.toMatchObject({
      code: closeCode,
    })
  },
)

it("ignores late and duplicate replies after a request has settled or the session has closed", async () => {
  const { session } = fixture()
  const first = session.request("server.health", {})
  const second = session.request("server.health", {})
  session.dispatchFrame({ type: "res", id: "2", ok: true, payload: { value: 2 } })
  session.dispatchFrame({ type: "res", id: "1", ok: true, payload: { value: 1 } })
  session.dispatchFrame({
    type: "res",
    id: "1",
    ok: true,
    payload: { value: "duplicate" },
  })
  expect(await first).toEqual({ value: 1 })
  expect(await second).toEqual({ value: 2 })
  const events = vi.fn()
  session.onEvent("unused", events)
  session.close()
  session.dispatchFrame({
    type: "event",
    event: "unused",
    payload: {},
    seq: 1,
    stateVersion: 0,
  })
  expect(events).not.toHaveBeenCalled()
  expect(vi.getTimerCount()).toBe(0)
})

it("a throwing or reentrant close observer cannot prevent pending-request or socket cleanup", async () => {
  const { session, close } = fixture()
  const pending = session.request("server.health", {}).catch((error) => error)
  const secondObserver = vi.fn(() => session.close())
  session.onClose(() => {
    throw new Error("consumer callback failed")
  })
  session.onClose(secondObserver)
  expect(() => session.close()).not.toThrow()
  expect(await pending).toMatchObject({ code: 1000 })
  expect(secondObserver).toHaveBeenCalledTimes(1)
  expect(close).toHaveBeenCalled()
  expect(vi.getTimerCount()).toBe(0)
})
