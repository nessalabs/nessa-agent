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
