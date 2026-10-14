// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import {
  applyReviewSnapshot,
  browserReviewNotifier,
  reviewToOpen,
  waitingReviewNotices,
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
  it("shows a notice when permission is already granted and does not ask", () => {
    const created: { title: string }[] = []
    class FakeNotification {
      static permission: NotificationPermission = "default"
      static requestPermission = vi.fn()
      onclick: (() => void) | null = null
      constructor(title: string) {
        created.push({ title })
      }
      close() {}
    }
    vi.stubGlobal("Notification", FakeNotification)
    const notifier = browserReviewNotifier(() => {})
    const held = review("p1")
    notifier.show(held)
    expect(FakeNotification.requestPermission).not.toHaveBeenCalled()
    expect(created).toEqual([])
    FakeNotification.permission = "granted"
    notifier.show(held)
    expect(created).toEqual([{ title: "Bash needs a decision" }])
    notifier.clear(held)
    vi.unstubAllGlobals()
  })

  it("shows a review dropped while permission was default once it is granted", () => {
    const created: { title: string }[] = []
    class FakeNotification {
      static permission: NotificationPermission = "default"
      constructor(title: string) {
        created.push({ title })
      }
      close() {}
    }
    vi.stubGlobal("Notification", FakeNotification)
    const notices = waitingReviewNotices(browserReviewNotifier(() => {}))
    const held = review("p1")
    notices.apply([held])
    expect(created).toEqual([])
    FakeNotification.permission = "granted"
    notices.showWaiting()
    expect(created).toEqual([{ title: "Bash needs a decision" }])
    notices.showWaiting()
    expect(created).toEqual([{ title: "Bash needs a decision" }])
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
