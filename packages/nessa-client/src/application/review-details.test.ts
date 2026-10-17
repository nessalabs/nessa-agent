import { expect, it, vi } from "vitest"
import type {
  ConversationPermission,
  ConversationReviewEntry,
} from "../generated/product.js"
import {
  reviewDetails,
  REVIEW_RETRY_FIRST_MS,
  REVIEW_RETRY_MOST_MS,
  type ReviewDetailsTimer,
} from "./review-details.js"

function entry(index: number): ConversationReviewEntry {
  return {
    review: index.toString(16).padStart(32, "0"),
    origin: "harness",
    toolName: "Write",
  }
}
function permission(id: string): ConversationPermission {
  return {
    executionId: "execution",
    permissionId: id,
    toolId: "tool",
    title: "Write",
    toolName: "Write",
    argumentsJson: "{}",
    origin: { kind: "harness" },
    ask: "tool",
    options: [{ id: "allow", label: "Allow", effect: "allow" }],
  }
}
/** A clock the test moves: each wait is held until `fire` runs it. */
function manualTimer() {
  const waits = new Map<number, { run: () => void; ms: number }>()
  let next = 0
  const timer: ReviewDetailsTimer = {
    set: (run, ms) => {
      waits.set(++next, { run, ms })
      return next
    },
    clear: (handle) => void waits.delete(handle as number),
  }
  return {
    timer,
    waits,
    fire() {
      const due = [...waits]
      waits.clear()
      for (const [, wait] of due) wait.run()
      return due.map(([, wait]) => wait.ms)
    },
  }
}
const settled = () => new Promise((resolve) => setTimeout(resolve, 0))

it("reads a review that failed again with a doubling wait while it is listed, and at once when asked", async () => {
  const clock = manualTimer()
  let failing = 6
  const read = vi.fn(async (review: string) => {
    if (failing-- > 0) throw new Error("unavailable")
    return permission(review)
  })
  const changed = vi.fn()
  const details = reviewDetails({ read, changed, timer: clock.timer })
  details.follow([entry(1)])
  expect(details.details().map((detail) => detail.state)).toEqual(["loading"])
  await settled()
  expect(details.details()[0]!.state).toBe("failed")
  const waited: number[] = []
  for (let attempt = 0; attempt < 5; attempt++) {
    waited.push(...clock.fire())
    await settled()
  }
  expect(waited).toEqual([1000, 2000, 4000, 8000, 16_000])
  expect(details.details()[0]!.state).toBe("failed")
  // A person asks again: at once, and the wait starts over.
  details.retry(entry(1).review)
  expect(details.details()[0]!.state).toBe("loading")
  expect(clock.waits.size).toBe(0)
  await settled()
  const loaded = details.details()[0]!
  expect(loaded.state).toBe("loaded")
  expect(loaded.state === "loaded" && loaded.permission.permissionId).toBe(
    entry(1).review,
  )
  expect(read).toHaveBeenCalledTimes(7)
  expect(clock.waits.size).toBe(0)
  details.dispose()
})

it("never waits longer than the most, and lets a review go once it is no longer listed", async () => {
  const clock = manualTimer()
  const read = vi.fn(
    async (_review: string, _signal: AbortSignal): Promise<ConversationPermission> => {
      throw new Error("unavailable")
    },
  )
  const details = reviewDetails({ read, changed: () => {}, timer: clock.timer })
  details.follow([entry(1)])
  await settled()
  let last = 0
  for (let attempt = 0; attempt < 10; attempt++) {
    last = clock.fire()[0] ?? last
    await settled()
  }
  expect(last).toBe(REVIEW_RETRY_MOST_MS)
  expect(REVIEW_RETRY_FIRST_MS).toBe(1000)
  // Answered elsewhere: not listed, not read again, nothing left waiting.
  details.follow([])
  expect(clock.waits.size).toBe(0)
  expect(details.details()).toEqual([])
  const reads = read.mock.calls.length
  clock.fire()
  await settled()
  expect(read).toHaveBeenCalledTimes(reads)
  details.dispose()
})

it("reads every listed review, the most a view lists, each once, and leaves nothing running when disposed", async () => {
  const clock = manualTimer()
  const signals: AbortSignal[] = []
  const read = vi.fn((review: string, signal: AbortSignal) => {
    signals.push(signal)
    return review === entry(0).review
      ? new Promise<ConversationPermission>(() => {})
      : Promise.resolve(permission(review))
  })
  const changed = vi.fn()
  const details = reviewDetails({ read, changed, timer: clock.timer })
  const listed = Array.from({ length: 144 }, (_, index) => entry(index))
  details.follow(listed)
  details.follow(listed)
  await settled()
  expect(read).toHaveBeenCalledTimes(144)
  expect(details.details().filter((detail) => detail.state === "loaded")).toHaveLength(
    143,
  )
  expect(details.get(entry(0).review)?.state).toBe("loading")
  details.dispose()
  expect(signals[0]!.aborted).toBe(true)
  expect(clock.waits.size).toBe(0)
  expect(details.details()).toEqual([])
})
