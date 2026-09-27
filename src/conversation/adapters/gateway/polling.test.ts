import { afterEach, expect, it, vi } from "vitest"
import { pollConversation } from "./polling"
afterEach(() => {
  vi.useRealTimers()
})
it("never overlaps reads and invalidates a late response when the tab unmounts", async () => {
  vi.useFakeTimers()
  let resolve!: () => void
  const pending = new Promise<void>((done) => {
    resolve = done
  })
  const read = vi.fn(() => pending)
  const invalidate = vi.fn()
  const stop = pollConversation(read, invalidate)
  await vi.advanceTimersByTimeAsync(5000)
  expect(read).toHaveBeenCalledTimes(1)
  stop()
  resolve()
  await vi.advanceTimersByTimeAsync(5000)
  expect(read).toHaveBeenCalledTimes(1)
  expect(invalidate).toHaveBeenCalledOnce()
})
it("refreshes immediately after a new mounted generation and uses the current delay", async () => {
  vi.useFakeTimers()
  let delay = 250
  const read = vi.fn(async () => {
    if (read.mock.calls.length === 2) delay = 2000
  })
  const stop = pollConversation(
    read,
    () => {},
    () => delay,
  )
  expect(read).toHaveBeenCalledOnce()
  await vi.advanceTimersByTimeAsync(250)
  expect(read).toHaveBeenCalledTimes(2)
  await vi.advanceTimersByTimeAsync(1999)
  expect(read).toHaveBeenCalledTimes(2)
  await vi.advanceTimersByTimeAsync(1)
  expect(read).toHaveBeenCalledTimes(3)
  stop()
  const next = pollConversation(read, () => {})
  expect(read).toHaveBeenCalledTimes(4)
  next()
})
