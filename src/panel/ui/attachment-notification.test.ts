// @vitest-environment jsdom
/**
 * What an attachment notice looks like once it is on screen.
 *
 * The rules about which notice to show are plain functions and tested beside
 * themselves. This is the part only the DOM can answer: that the words reach a
 * screen reader at all now that they are no longer a `role="alert"` paragraph,
 * and that the action offered is the one the notice asked for — a Retry for
 * uploads, the file picker only where choosing again could help, and no control
 * at all for a refusal nothing can be done about.
 *
 * Announcement is politer than it was: the notification's own status region is
 * `aria-live="polite"`, where the paragraph it replaces was an assertive alert.
 * That is the surface every other notice in this pane already announces through.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import type { AttachmentNotice } from "../application/attachment-notice"
import { AttachmentNotification } from "./attachment-notification"

const retryUploads = vi.fn()
const chooseFiles = vi.fn()
const dismiss = vi.fn()

let container: HTMLDivElement
let root: Root

async function show(notice: AttachmentNotice) {
  await React.act(async () => {
    root.render(
      React.createElement(AttachmentNotification, {
        notice,
        onRetryUploads: retryUploads,
        onChooseFiles: chooseFiles,
        onDismiss: dismiss,
      }),
    )
  })
}

/** The one live region the notification announces through. */
const announced = () => container.querySelector("[aria-live]")
const button = (label: string) => container.querySelector(`[aria-label="${label}"]`)

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom has no `matchMedia`, and the notification asks it about reduced
  // motion before it plays its exit. Answering "no preference" is the harder
  // of the two answers: it is the one that would animate, if jsdom had the
  // Web Animations API to animate with.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  vi.clearAllMocks()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("announces the title and the line through a live region", async () => {
  await show({
    title: "File is too large",
    description: "holiday.mp4 is over 64 MiB, the most one attachment can weigh.",
    action: null,
    dismissible: true,
  })
  const region = announced()
  expect(region?.getAttribute("aria-live")).toBe("polite")
  // Atomic, so the heading and the line are read as one thing rather than
  // whichever half changed.
  expect(region?.getAttribute("aria-atomic")).toBe("true")
  expect(region?.textContent).toContain("File is too large")
  expect(region?.textContent).toContain("64 MiB")
  // Nothing to be done about the weight, so nothing is offered — above all not
  // the picker, which holds the file to the same bound.
  expect(button("Choose files")).toBeNull()
  expect(button("Retry")).toBeNull()
  // A refusal is the panel's own memory, and can be put away.
  expect(button("Dismiss notification")).not.toBeNull()
})

it("offers the picker, and opens it, where choosing the files again could help", async () => {
  await show({
    title: "Files could not be attached",
    description: "Choose them again.",
    action: { kind: "choose-files" },
    dismissible: true,
  })
  const choose = button("Choose files")
  expect(choose).not.toBeNull()
  await React.act(async () => {
    ;(choose as HTMLButtonElement).click()
  })
  expect(chooseFiles).toHaveBeenCalledOnce()
  expect(retryUploads).not.toHaveBeenCalled()
})

it("retries exactly the uploads the notice named, and cannot be put away", async () => {
  await show({
    title: "2 images didn't upload",
    description: "Couldn't reach the gateway.",
    action: { kind: "retry-uploads", files: ["b", "c"] },
    dismissible: false,
  })
  // The draft still holds the failed files; dismissing would leave nothing
  // above the composer saying so.
  expect(button("Dismiss notification")).toBeNull()
  const retry = button("Retry")
  expect(retry).not.toBeNull()
  await React.act(async () => {
    ;(retry as HTMLButtonElement).click()
  })
  expect(retryUploads).toHaveBeenCalledExactlyOnceWith(["b", "c"])
})

it("puts a refusal away when asked", async () => {
  await show({
    title: "Too many files",
    description: "A draft holds up to 20 files.",
    action: null,
    dismissible: true,
  })
  await React.act(async () => {
    ;(button("Dismiss notification") as HTMLButtonElement).click()
  })
  // jsdom implements neither `Element.animate` nor the Web Animations API the
  // notification's exit uses, so the component takes its own reduced-motion
  // path and calls back at once. What the animated exit looks like is not
  // reachable from here; see issue #89.
  expect(dismiss).toHaveBeenCalledOnce()
})
