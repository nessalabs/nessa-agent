// @vitest-environment jsdom
/**
 * Settings › Access review notifications: the row rests where the browser
 * has no notifications, and Allow is what asks for permission.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  bindNoticeAccess,
  bindWaitingReviewNotices,
  browserNoticeAccess,
  type NoticeAccess,
} from "../../workspace/adapters/gateway/review-notices"
import { ReviewNoticeRows } from "./review-notice-rows"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  bindWaitingReviewNotices(() => {})
  bindNoticeAccess(browserNoticeAccess)
  await act(async () => root.unmount())
  host.remove()
  vi.unstubAllGlobals()
})

it("rests where this window cannot notify", async () => {
  await act(async () => root.render(<ReviewNoticeRows />))
  expect(host.textContent).toBe("")
})

it("asks for permission from Allow", async () => {
  class FakeNotification {
    static permission: NotificationPermission = "default"
    static requestPermission(): Promise<NotificationPermission> {
      FakeNotification.permission = "granted"
      return Promise.resolve("granted")
    }
  }
  const requestPermission = vi.spyOn(FakeNotification, "requestPermission")
  vi.stubGlobal("Notification", FakeNotification)
  await act(async () => root.render(<ReviewNoticeRows />))
  expect(requestPermission).not.toHaveBeenCalled()
  const button = host.querySelector("button")
  expect(button?.textContent).toBe("Allow")
  await act(async () => button?.click())
  expect(requestPermission).toHaveBeenCalledOnce()
  expect(host.textContent).toContain("On. A review that is waiting shows a notification.")
})

it("notifies for a waiting review when Allow is granted", async () => {
  class FakeNotification {
    static permission: NotificationPermission = "default"
    static requestPermission(): Promise<NotificationPermission> {
      FakeNotification.permission = "granted"
      return Promise.resolve("granted")
    }
  }
  vi.stubGlobal("Notification", FakeNotification)
  const show = vi.fn()
  bindWaitingReviewNotices(show)
  await act(async () => root.render(<ReviewNoticeRows />))
  const button = host.querySelector("button")
  await act(async () => button?.click())
  expect(show).toHaveBeenCalledOnce()
})

it("asks the host where the host shows the notices", async () => {
  let access: NoticeAccess = "not-asked"
  const ask = vi.fn(() => {
    access = "allowed"
    return Promise.resolve(access)
  })
  bindNoticeAccess({
    read: () => Promise.resolve(access),
    ask,
    blocked: "Blocked by the host.",
  })
  const show = vi.fn()
  bindWaitingReviewNotices(show)
  await act(async () => root.render(<ReviewNoticeRows />))
  expect(ask).not.toHaveBeenCalled()
  await act(async () => host.querySelector("button")?.click())
  expect(ask).toHaveBeenCalledOnce()
  expect(show).toHaveBeenCalledOnce()
  expect(host.textContent).toContain("On. A review that is waiting shows a notification.")
})

it("says where the host's notices were blocked", async () => {
  bindNoticeAccess({
    read: () => Promise.resolve("denied"),
    ask: () => Promise.resolve("denied"),
    blocked: "Blocked by the host.",
  })
  await act(async () => root.render(<ReviewNoticeRows />))
  expect(host.textContent).toContain("Blocked by the host.")
  expect(host.querySelector("button")).toBeNull()
})

it("admits one outstanding browser permission API request across separate clicks", async () => {
  class FakeNotification {
    static permission: NotificationPermission = "default"
    static requestPermission = vi.fn(() => new Promise<NotificationPermission>(() => {}))
  }
  vi.stubGlobal("Notification", FakeNotification)
  bindNoticeAccess(browserNoticeAccess)
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  await act(async () => allow().click())
  expect(FakeNotification.requestPermission).toHaveBeenCalledTimes(1)
})

it("does not let a retired Allow completion invoke the replacement replay binding", async () => {
  let finish!: (value: NoticeAccess) => void
  const showA = vi.fn()
  const showB = vi.fn()
  bindNoticeAccess({
    read: async () => "not-asked",
    ask: () =>
      new Promise((resolve) => {
        finish = resolve
      }),
    blocked: "A blocked",
  })
  bindWaitingReviewNotices(showA)
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  bindNoticeAccess({
    read: async () => "not-asked",
    ask: async () => "denied",
    blocked: "B blocked",
  })
  bindWaitingReviewNotices(showB)
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => finish("allowed"))
  expect(showA).not.toHaveBeenCalled()
  expect(showB).not.toHaveBeenCalled()
  expect(host.querySelector("button")?.textContent).toBe("Allow")
})

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

it("admits one Allow request for same-task clicks and retries after rejection", async () => {
  const first = deferred<NoticeAccess>()
  const ask = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValueOnce("allowed")
  const show = vi.fn()
  bindNoticeAccess({ read: async () => "not-asked", ask, blocked: "Blocked" })
  bindWaitingReviewNotices(show)
  await act(async () => root.render(<ReviewNoticeRows />))
  const button = allow()
  await act(async () => {
    button.click()
    button.click()
  })
  expect(ask).toHaveBeenCalledTimes(1)
  expect(button.disabled).toBe(true)
  await act(async () => first.reject(new Error("prompt unavailable")))
  expect(button.disabled).toBe(false)
  await act(async () => button.click())
  expect(ask).toHaveBeenCalledTimes(2)
  expect(show).toHaveBeenCalledTimes(1)
})

it.each(["not-asked", "denied", "unsupported"] as const)(
  "settles a %s prompt without replaying reviews",
  async (answer) => {
    const prompt = deferred<NoticeAccess>()
    const show = vi.fn()
    bindNoticeAccess({
      read: async () => "not-asked",
      ask: () => prompt.promise,
      blocked: "Blocked by test host",
    })
    bindWaitingReviewNotices(show)
    await act(async () => root.render(<ReviewNoticeRows />))
    await act(async () => allow().click())
    await act(async () => prompt.resolve(answer))
    expect(show).not.toHaveBeenCalled()
    if (answer === "not-asked") expect(allow().disabled).toBe(false)
    else expect(host.querySelector("button")).toBeNull()
    if (answer === "denied") expect(host.textContent).toContain("Blocked by test host")
    if (answer === "unsupported") expect(host.textContent).toBe("")
  },
)

it("ignores a retired prompt after unmount even when the permission source is reused", async () => {
  const oldPrompt = deferred<NoticeAccess>()
  const currentPrompt = deferred<NoticeAccess>()
  const ask = vi
    .fn()
    .mockReturnValueOnce(oldPrompt.promise)
    .mockReturnValueOnce(currentPrompt.promise)
  const show = vi.fn()
  bindNoticeAccess({ read: async () => "not-asked", ask, blocked: "Blocked" })
  bindWaitingReviewNotices(show)
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  await act(async () => root.render(null))
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  await act(async () => oldPrompt.resolve("allowed"))
  expect(show).not.toHaveBeenCalled()
  expect(allow().disabled).toBe(true)
  await act(async () => currentPrompt.resolve("allowed"))
  expect(show).toHaveBeenCalledTimes(1)
  expect(host.textContent).toContain("On.")
})

it("does not replay into a rebound source even before its next render", async () => {
  const prompt = deferred<NoticeAccess>()
  const show = vi.fn()
  bindNoticeAccess({
    read: async () => "not-asked",
    ask: () => prompt.promise,
    blocked: "A",
  })
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  bindNoticeAccess({
    read: async () => "denied",
    ask: async () => "denied",
    blocked: "B",
  })
  bindWaitingReviewNotices(show)
  await act(async () => prompt.resolve("allowed"))
  expect(show).not.toHaveBeenCalled()
  await act(async () => root.render(<ReviewNoticeRows />))
  expect(host.textContent).toContain("B")
  expect(host.querySelector("button")).toBeNull()
})

it("ignores a retired initial read after a replacement permission grant", async () => {
  const read = deferred<NoticeAccess>()
  bindNoticeAccess({ read: () => read.promise, ask: async () => "denied", blocked: "A" })
  await act(async () => root.render(<ReviewNoticeRows />))
  bindNoticeAccess({
    read: async () => "not-asked",
    ask: async () => "allowed",
    blocked: "B",
  })
  const show = vi.fn()
  bindWaitingReviewNotices(show)
  await act(async () => root.render(<ReviewNoticeRows />))
  await act(async () => allow().click())
  await act(async () => read.resolve("denied"))
  expect(host.textContent).toContain("On.")
  expect(show).toHaveBeenCalledTimes(1)
})

function allow() {
  const button = host.querySelector("button")
  if (!button) throw new Error("Missing Allow button")
  return button
}
