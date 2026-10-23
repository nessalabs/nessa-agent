import { expect, it, vi } from "vitest"
import { maintainBrowserSession } from "./browser-renewal"
it("renews visible sessions, retries outages, and ignores callbacks after disposal", async () => {
  let tick = () => {}
  let visible = false
  const ended = vi.fn()
  const check = vi
    .fn<() => Promise<boolean>>()
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValue(false)
  const cancel = vi.fn()
  const stop = maintainBrowserSession({
    check,
    ended,
    visible: () => visible,
    subscribe: () => cancel,
    schedule: (callback) => {
      tick = callback
      return cancel
    },
  })
  tick()
  expect(check).not.toHaveBeenCalled()
  visible = true
  tick()
  tick()
  expect(check).toHaveBeenCalledTimes(1)
  await vi.waitFor(() => expect(check).toHaveBeenCalledTimes(1))
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
  expect(ended).not.toHaveBeenCalled()
  tick()
  await vi.waitFor(() => expect(ended).toHaveBeenCalledTimes(1))
  stop()
  tick()
  expect(check).toHaveBeenCalledTimes(2)
  expect(cancel).toHaveBeenCalledTimes(2)
})
it("does not end a new scope when an old check completes after disposal", async () => {
  let tick = () => {}
  let resolve: (valid: boolean) => void = () => {}
  const ended = vi.fn()
  const stop = maintainBrowserSession({
    check: () =>
      new Promise<boolean>((r) => {
        resolve = r
      }),
    ended,
    visible: () => true,
    subscribe: () => () => {},
    schedule: (callback) => {
      tick = callback
      return () => {}
    },
  })
  tick()
  stop()
  resolve(false)
  await Promise.resolve()
  expect(ended).not.toHaveBeenCalled()
})

it.each([true, false, "outage"] as const)(
  "serializes visibility and scheduled checks while a %s result is pending",
  async (result) => {
    let onVisible = () => {},
      tick = () => {}
    let resolve!: (valid: boolean) => void
    let reject!: (error: Error) => void
    const checked = new Promise<boolean>((done, fail) => {
      resolve = done
      reject = fail
    })
    const check = vi
      .fn<() => Promise<boolean>>()
      .mockReturnValueOnce(checked)
      .mockResolvedValue(true)
    const ended = vi.fn(),
      unsubscribe = vi.fn(),
      cancel = vi.fn()
    const stop = maintainBrowserSession({
      check,
      ended,
      visible: () => true,
      subscribe: (callback) => {
        onVisible = callback
        return unsubscribe
      },
      schedule: (callback) => {
        tick = callback
        return cancel
      },
    })
    onVisible()
    for (let i = 0; i < 10; i++) {
      tick()
      onVisible()
    }
    expect(check).toHaveBeenCalledOnce()
    if (result === "outage") reject(new Error("service unreachable"))
    else resolve(result)
    for (let i = 0; i < 6; i++) await Promise.resolve()
    expect(ended).toHaveBeenCalledTimes(result === false ? 1 : 0)
    tick()
    for (let i = 0; i < 6; i++) await Promise.resolve()
    expect(check).toHaveBeenCalledTimes(2)
    stop()
    onVisible()
    tick()
    expect(check).toHaveBeenCalledTimes(2)
    expect(unsubscribe).toHaveBeenCalledOnce()
    expect(cancel).toHaveBeenCalledOnce()
  },
)

it.each([true, false, "outage"] as const)(
  "releases both triggers and ignores a late %s renewal outcome",
  async (result) => {
    let trigger = () => {}
    let resolve!: (valid: boolean) => void
    let reject!: (error: Error) => void
    const check = vi.fn(
      () =>
        new Promise<boolean>((done, fail) => {
          resolve = done
          reject = fail
        }),
    )
    const ended = vi.fn(),
      unsubscribe = vi.fn(),
      cancel = vi.fn()
    const stop = maintainBrowserSession({
      check,
      ended,
      visible: () => true,
      subscribe: (callback) => {
        trigger = callback
        return unsubscribe
      },
      schedule: () => cancel,
    })
    trigger()
    stop()
    if (result === "outage") reject(new Error("late network failure"))
    else resolve(result)
    for (let i = 0; i < 6; i++) await Promise.resolve()
    trigger()
    expect(check).toHaveBeenCalledOnce()
    expect(ended).not.toHaveBeenCalled()
    expect(unsubscribe).toHaveBeenCalledOnce()
    expect(cancel).toHaveBeenCalledOnce()
  },
)
