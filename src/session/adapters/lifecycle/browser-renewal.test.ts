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
