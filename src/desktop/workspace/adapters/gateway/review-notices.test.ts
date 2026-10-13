import { describe, expect, it, vi } from "vitest"
import {
  applyReviewSnapshot,
  reviewToOpen,
  waitingReviews,
  type ReviewNotifier,
  type WaitingReview,
} from "./review-notices"

const review = (permissionId: string, tool = "Bash"): WaitingReview => ({
  conversationId: "11111111-1111-4111-8111-111111111111",
  executionId: "turn",
  permissionId,
  tool,
})

describe("waitingReviews", () => {
  it("reads a snapshot and rejects a payload that is not one", () => {
    expect(waitingReviews({ reviews: [review("p1")] })).toEqual([review("p1")])
    expect(waitingReviews({ reviews: [] })).toEqual([])
    expect(waitingReviews({})).toBeNull()
    expect(waitingReviews({ reviews: [{ conversationId: "c" }] })).toBeNull()
    expect(waitingReviews({ reviews: "p1" })).toBeNull()
  })
})

describe("applyReviewSnapshot", () => {
  it("shows a review that arrived and clears one that left", () => {
    const show = vi.fn()
    const clear = vi.fn()
    const notifier: ReviewNotifier = { show, clear }
    const first = review("p1")
    const second = review("p2", "Read")
    applyReviewSnapshot([], [first], notifier)
    applyReviewSnapshot([first], [first, second], notifier)
    applyReviewSnapshot([first, second], [], notifier)
    expect(show.mock.calls.map((call) => call[0].permissionId)).toEqual(["p1", "p2"])
    expect(clear.mock.calls.map((call) => call[0].permissionId)).toEqual(["p1", "p2"])
  })
})

describe("reviewToOpen", () => {
  it("reads the conversation a notice click names", () => {
    const event = new CustomEvent("nessa-open-review", { detail: "conversation" })
    expect(reviewToOpen(event)).toBe("conversation")
    expect(reviewToOpen(new CustomEvent("nessa-open-review", { detail: "" }))).toBeNull()
    expect(reviewToOpen(new Event("click"))).toBeNull()
  })
})
