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

it("a throwing event subscriber does not suppress another subscriber's update", () => {
  const { session } = fixture()
  const received = vi.fn()
  session.onEvent("conversation.listed", () => {
    throw new Error("consumer failed")
  })
  session.onEvent("conversation.listed", received)
  expect(() =>
    session.dispatchFrame({
      type: "event",
      event: "conversation.listed",
      payload: { conversationId: "chat" },
      seq: 1,
      stateVersion: 0,
    }),
  ).not.toThrow()
  expect(received).toHaveBeenCalledWith({ conversationId: "chat" })
  session.close()
})

it("event subscriptions added during delivery start with the next event", () => {
  const { session } = fixture()
  const late = vi.fn()
  session.onEvent("conversation.listed", () => {
    session.onEvent("conversation.listed", late)
  })
  const frame = {
    type: "event" as const,
    event: "conversation.listed",
    payload: {},
    seq: 1,
    stateVersion: 0,
  }
  session.dispatchFrame(frame)
  expect(late).not.toHaveBeenCalled()
  session.dispatchFrame(frame)
  expect(late).toHaveBeenCalledOnce()
  session.close()
})

it("an event subscriber removed before its turn does not receive the event", () => {
  const { session } = fixture()
  const received = vi.fn()
  session.onEvent("conversation.listed", () => off())
  const off = session.onEvent("conversation.listed", received)
  session.dispatchFrame({
    type: "event",
    event: "conversation.listed",
    payload: {},
    seq: 1,
    stateVersion: 0,
  })
  expect(received).not.toHaveBeenCalled()
  session.close()
})

it("a removed and re-added event callback starts with the next event in registration order", () => {
  const { session } = fixture()
  const received: string[] = []
  const later = () => received.push("later")
  let replace = true
  session.onEvent("conversation.listed", () => {
    received.push("first")
    if (!replace) return
    replace = false
    off()
    session.onEvent("conversation.listed", later)
  })
  const off = session.onEvent("conversation.listed", later)
  session.onEvent("conversation.listed", () => received.push("last"))
  const frame = {
    type: "event" as const,
    event: "conversation.listed",
    payload: {},
    seq: 1,
    stateVersion: 0,
  }
  session.dispatchFrame(frame)
  expect(received).toEqual(["first", "last"])
  received.length = 0
  session.dispatchFrame(frame)
  expect(received).toEqual(["first", "last", "later"])
  session.close()
})

it("an old unsubscribe handle cannot remove a replacement registration of the same function", () => {
  const { session } = fixture()
  const received = vi.fn()
  const oldOff = session.onEvent("conversation.listed", received)
  oldOff()
  const newOff = session.onEvent("conversation.listed", received)
  oldOff()
  const frame = {
    type: "event" as const,
    event: "conversation.listed",
    payload: {},
    seq: 1,
    stateVersion: 0,
  }
  session.dispatchFrame(frame)
  expect(received).toHaveBeenCalledOnce()
  newOff()
  session.dispatchFrame(frame)
  expect(received).toHaveBeenCalledOnce()
  session.close()
})

it("duplicate active subscriptions of one function keep one delivery and share removal", () => {
  const { session } = fixture()
  const received = vi.fn()
  const firstOff = session.onEvent("conversation.listed", received)
  const secondOff = session.onEvent("conversation.listed", received)
  const frame = {
    type: "event" as const,
    event: "conversation.listed",
    payload: {},
    seq: 1,
    stateVersion: 0,
  }
  session.dispatchFrame(frame)
  expect(received).toHaveBeenCalledOnce()
  secondOff()
  const replacementOff = session.onEvent("conversation.listed", received)
  firstOff()
  secondOff()
  session.dispatchFrame(frame)
  expect(received).toHaveBeenCalledTimes(2)
  replacementOff()
  session.dispatchFrame(frame)
  expect(received).toHaveBeenCalledTimes(2)
  session.close()
})

it("a replacement registration receives a nested event but not its interrupted outer event", () => {
  const { session } = fixture()
  const received = vi.fn()
  const outer = {
    type: "event" as const,
    event: "conversation.listed",
    payload: { name: "outer" },
    seq: 1,
    stateVersion: 0,
  }
  const nested = { ...outer, payload: { name: "nested" }, seq: 2 }
  let replace = true
  session.onEvent("conversation.listed", () => {
    if (!replace) return
    replace = false
    off()
    session.onEvent("conversation.listed", received)
    session.dispatchFrame(nested)
  })
  const off = session.onEvent("conversation.listed", received)
  session.dispatchFrame(outer)
  expect(received.mock.calls).toEqual([[{ name: "nested" }]])
  session.close()
})

it.each(["client", "transport"] as const)(
  "%s closure during an event stops remaining and later event delivery",
  (closure) => {
    const { session } = fixture()
    const received = vi.fn()
    session.onEvent("conversation.listed", () => {
      if (closure === "client") session.close()
      else session.dispatchClose(1006, "connection lost")
    })
    session.onEvent("conversation.listed", received)
    const frame = {
      type: "event" as const,
      event: "conversation.listed",
      payload: {},
      seq: 1,
      stateVersion: 0,
    }
    session.dispatchFrame(frame)
    session.onEvent("conversation.listed", received)
    session.dispatchFrame(frame)
    expect(received).not.toHaveBeenCalled()
    expect(session.termination?.code).toBe(closure === "client" ? 1000 : 1006)
  },
)

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

it("a late waiting-review subscriber is handed the set the socket already received", async () => {
  const { session } = fixture()
  const waiting = (permissionId: string) => ({
    type: "event" as const,
    event: "permission.reviewWaiting",
    payload: { reviews: [{ permissionId }] },
    seq: 1,
    stateVersion: 0,
  })
  // The gateway sends the set once the socket is authenticated, before the
  // window has subscribed.
  session.dispatchFrame(waiting("first"))
  session.dispatchFrame(waiting("second"))
  const late = vi.fn()
  session.onEvent("permission.reviewWaiting", late)
  expect(late).not.toHaveBeenCalled()
  await Promise.resolve()
  expect(late.mock.calls).toEqual([[{ reviews: [{ permissionId: "second" }] }]])

  // A subscriber removed before the hand-over gets nothing; another event
  // is not kept.
  const removed = vi.fn()
  session.onEvent("permission.reviewWaiting", removed)()
  session.dispatchFrame({ ...waiting("x"), event: "conversation.listed" })
  const listed = vi.fn()
  session.onEvent("conversation.listed", listed)
  await Promise.resolve()
  expect(removed).not.toHaveBeenCalled()
  expect(listed).not.toHaveBeenCalled()
  session.close()
})
