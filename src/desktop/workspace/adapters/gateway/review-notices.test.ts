// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import {
  applyReviewSnapshot,
  bindConversationTitles,
  browserNoticeAccess,
  browserReviewNotifier,
  hostReviewNotices,
  reviewToOpen,
  waitingReviewNotices,
  waitingReviews,
  type HostNotices,
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

  it("keeps the conversation's title the gateway sent, and drops a blank one", () => {
    expect(waitingReviews({ reviews: [{ ...review("p1"), title: " Fix CI " }] })).toEqual(
      [{ ...review("p1"), conversationTitle: "Fix CI" }],
    )
    expect(waitingReviews({ reviews: [{ ...review("p1"), title: "  " }] })).toEqual([
      review("p1"),
    ])
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

describe("browserReviewNotifier names the conversation", () => {
  it("uses the conversation title when the window has one, and the id otherwise", () => {
    const created: { title: string; body: string }[] = []
    class FakeNotification {
      static permission: NotificationPermission = "granted"
      constructor(title: string, options?: NotificationOptions) {
        created.push({ title, body: options?.body ?? "" })
      }
      close() {}
    }
    vi.stubGlobal("Notification", FakeNotification)
    const notifier = browserReviewNotifier(() => {})
    const held = review("p1")
    notifier.show(held)
    expect(created).toEqual([
      {
        title: "Bash needs a decision",
        body: "11111111-1111-4111-8111-111111111111",
      },
    ])
    bindConversationTitles(() => "Reconnect tests")
    notifier.show(review("p2"))
    expect(created[1]).toEqual({
      title: "Bash needs a decision",
      body: "Reconnect tests",
    })
    bindConversationTitles(() => "   ")
    notifier.show(review("p3"))
    expect(created[2]?.body).toBe("11111111-1111-4111-8111-111111111111")
    bindConversationTitles(() => null)
    vi.unstubAllGlobals()
  })

  it("prefers the gateway's title, which a review can carry before the window lists it", () => {
    const created: string[] = []
    class FakeNotification {
      static permission: NotificationPermission = "granted"
      constructor(_title: string, options?: NotificationOptions) {
        created.push(options?.body ?? "")
      }
      close() {}
    }
    vi.stubGlobal("Notification", FakeNotification)
    const notifier = browserReviewNotifier(() => {})
    notifier.show({ ...review("p1"), conversationTitle: "Plan the trip" })
    bindConversationTitles(() => "Older title")
    notifier.show({ ...review("p2"), conversationTitle: "Plan the trip" })
    expect(created).toEqual(["Plan the trip", "Plan the trip"])
    bindConversationTitles(() => null)
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

describe("hostReviewNotices", () => {
  it("sends each whole set, the newest after the one in flight", async () => {
    const sent: string[][] = []
    const finish: (() => void)[] = []
    const host: HostNotices = {
      sync: (notices) => {
        sent.push(
          notices.map(
            (notice) => `${notice.permissionId} ${notice.title} / ${notice.body}`,
          ),
        )
        return new Promise((resolve) => finish.push(resolve))
      },
    }
    bindConversationTitles(() => "Fix the build")
    const notices = hostReviewNotices(host)
    notices.apply([review("p1")])
    // Two more sets while the first is in flight: only the newest follows.
    notices.apply([review("p1"), review("p2")])
    notices.apply([])
    expect(sent).toEqual([["p1 Bash needs a decision / Fix the build"]])
    finish[0]?.()
    await vi.waitFor(() => expect(sent).toHaveLength(2))
    expect(sent[1]).toEqual([])
    finish[1]?.()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(sent).toHaveLength(2)
    bindConversationTitles(() => null)
  })

  it("sends the newest set again when asked, and after a refused call", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const sent: number[] = []
    let refuse = true
    const notices = hostReviewNotices({
      sync: (waiting) => {
        sent.push(waiting.length)
        return refuse ? Promise.reject(new Error("no host")) : Promise.resolve()
      },
    })
    notices.apply([review("p1")])
    await vi.waitFor(() => expect(warn).toHaveBeenCalledOnce())
    refuse = false
    notices.showWaiting()
    await vi.waitFor(() => expect(sent).toEqual([1, 1]))
    warn.mockRestore()
  })
})

describe("browserNoticeAccess", () => {
  it("reads and asks the browser's permission", async () => {
    vi.stubGlobal("Notification", undefined)
    await expect(browserNoticeAccess.read()).resolves.toBe("unsupported")
    await expect(browserNoticeAccess.ask()).resolves.toBe("unsupported")
    class FakeNotification {
      static permission: NotificationPermission = "default"
      static requestPermission(): Promise<NotificationPermission> {
        return Promise.resolve("denied")
      }
    }
    vi.stubGlobal("Notification", FakeNotification)
    await expect(browserNoticeAccess.read()).resolves.toBe("not-asked")
    await expect(browserNoticeAccess.ask()).resolves.toBe("denied")
    FakeNotification.permission = "granted"
    await expect(browserNoticeAccess.read()).resolves.toBe("allowed")
    vi.unstubAllGlobals()
  })
})
