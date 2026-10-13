// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import {
  applyReviewSnapshot,
  browserReviewNotifier,
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

describe("browserReviewNotifier", () => {
  it("does not ask for permission until a pointer or key press", async () => {
    const created: { title: string }[] = []
    class FakeNotification {
      static permission: NotificationPermission = "default"
      static requestPermission(): Promise<NotificationPermission> {
        FakeNotification.permission = "granted"
        return Promise.resolve("granted")
      }
      onclick: (() => void) | null = null
      constructor(title: string) {
        created.push({ title })
      }
      close() {}
    }
    const requestPermission = vi.spyOn(FakeNotification, "requestPermission")
    vi.stubGlobal("Notification", FakeNotification)
    const notifier = browserReviewNotifier(() => {})
    const held = review("p1")
    notifier.show(held)
    expect(requestPermission).not.toHaveBeenCalled()
    expect(created).toEqual([])
    window.dispatchEvent(new Event("pointerdown"))
    await Promise.resolve()
    expect(requestPermission).toHaveBeenCalledOnce()
    expect(created).toEqual([{ title: "Bash needs a decision" }])
    notifier.clear(held)
    vi.unstubAllGlobals()
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
