// @vitest-environment jsdom
/**
 * What the composer's attachment notices look like once they are on screen.
 *
 * Which notices there are is a plain function, tested beside itself. This is
 * the part only the DOM can answer: that the words reach a screen reader at all
 * now that they are no longer a `role="alert"` paragraph, that two true things
 * are two notices rather than one winning, and that the action offered is the
 * one the notice asked for — a Retry for uploads, the file picker only where
 * choosing again could help, and no control at all for a refusal nothing can be
 * done about.
 *
 * This component is also where the composition lives, so that there is nowhere
 * above it for a gate to come back: `app.tsx` passes it the refusal and the
 * files and renders whatever it returns.
 *
 * Announcement is politer than it was: the notification's own status region is
 * `aria-live="polite"`, where the paragraph it replaces was an assertive alert.
 * That is the surface every other notice in this pane already announces through.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The component reads one pure rule from the conversation. Its barrel also
// exports the conversation's components, which need the whole design system
// resolved; `testing` is the same rules without them.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import type { FileAttachment } from "../../conversation/testing"
import type { AttachmentRefusal } from "../application/attachment-notice"
import { AttachmentNotices, AttachmentReadingStatus } from "./attachment-notices"

const retryUploads = vi.fn()
const chooseFiles = vi.fn()
const dismissRefusal = vi.fn()

const file = (
  id: string,
  upload: FileAttachment["upload"],
  mimeType = "image/png",
): FileAttachment => ({
  type: "file",
  id,
  name: `${id}.png`,
  mimeType,
  size: 3,
  previewUrl: "blob:test",
  path: null,
  upload,
})

let container: HTMLDivElement
let root: Root

async function show(input: {
  refusal?: AttachmentRefusal | null
  files?: readonly FileAttachment[]
  imageInput?: boolean | undefined
}) {
  await React.act(async () => {
    root.render(
      React.createElement(AttachmentNotices, {
        canChoosePaths: true,
        refusal: input.refusal ?? null,
        files: input.files ?? [],
        imageInput: input.imageInput,
        onRetryUploads: retryUploads,
        onChooseFiles: chooseFiles,
        onDismissRefusal: dismissRefusal,
      }),
    )
  })
}

/** Each notice's own live region, in the order they are read down the screen. */
const announced = () =>
  [...container.querySelectorAll("[aria-live]")].map((region) => region.textContent ?? "")
const buttons = (label: string) => [
  ...container.querySelectorAll<HTMLButtonElement>(`[aria-label="${label}"]`),
]
const button = (label: string) => buttons(label)[0]

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

it("says nothing at all when there is nothing to say", async () => {
  await show({})
  expect(announced()).toEqual([])
})

it("announces the title and the line through a live region", async () => {
  await show({
    refusal: {
      reason: "file-too-large",
      files: [{ name: "holiday.mp4", type: "video/mp4" }],
    },
  })
  const region = container.querySelector("[aria-live]")
  expect(region?.getAttribute("aria-live")).toBe("polite")
  // Atomic, so the heading and the line are read as one thing rather than
  // whichever half changed.
  expect(region?.getAttribute("aria-atomic")).toBe("true")
  expect(region?.textContent).toContain("File is too large")
  expect(region?.textContent).toContain("64 MiB")
  // A video does not have to be carried at all, so the picker is a real answer
  // to its weight and is offered. Nothing else is.
  expect(buttons("Choose files")).toHaveLength(1)
  expect(buttons("Retry")).toHaveLength(0)
  // A refusal is the panel's own memory, and can be put away.
  expect(button("Dismiss notification")).toBeDefined()
})

it("shows a refusal and a failed upload together, each keeping its own action", async () => {
  // The regression. Whichever way round the two used to be ranked, one of them
  // vanished: first the upload's notice and its Retry, then — had the order
  // simply been reversed — a 720 MB file refused with nothing on screen moving.
  await show({
    refusal: {
      reason: "file-too-large",
      files: [{ name: "holiday.mp4", type: "video/mp4" }],
    },
    files: [file("b", { status: "failed", reason: "unavailable" })],
    imageInput: true,
  })
  const [draft, refusal] = announced()
  expect(draft).toContain("Image didn't upload")
  expect(refusal).toContain("File is too large")
  expect(refusal).toContain("holiday.mp4")

  // The Retry belongs to the draft's notice and still works.
  await React.act(async () => {
    button("Retry")!.click()
  })
  expect(retryUploads).toHaveBeenCalledExactlyOnceWith(["b"])

  // Exactly one of the two can be put away, and it is the refusal.
  expect(buttons("Dismiss notification")).toHaveLength(1)
  await React.act(async () => {
    button("Dismiss notification")!.click()
  })
  // jsdom implements neither `Element.animate` nor the Web Animations API the
  // notification's exit uses, so the component takes its own reduced-motion
  // path and calls back at once. What the animated exit looks like is not
  // reachable from here; see issue #89.
  expect(dismissRefusal).toHaveBeenCalledOnce()
})

it("offers the picker, and opens it, where choosing the files again could help", async () => {
  await show({ refusal: { reason: "unreadable-files" } })
  const choose = button("Choose files")
  expect(choose).toBeDefined()
  await React.act(async () => {
    choose!.click()
  })
  expect(chooseFiles).toHaveBeenCalledOnce()
  expect(retryUploads).not.toHaveBeenCalled()
})

it("does not let a fact about the draft be put away", async () => {
  await show({ files: [file("n", { status: "not-started" }, "application/pdf")] })
  expect(announced()[0]).toContain("File can't be sent")
  // The draft still holds the file; dismissing would leave nothing above the
  // composer saying so.
  expect(buttons("Dismiss notification")).toHaveLength(0)
})

it("has the reading region on the page before the reading starts", async () => {
  // A live region inserted with its text already in it is announced by
  // nothing, which is what was wrong with the paragraph this replaces. So the
  // region is mounted empty and the text arrives into the same node.
  await React.act(async () => {
    root.render(React.createElement(AttachmentReadingStatus, { reading: false }))
  })
  const region = container.querySelector("[aria-live]")
  expect(region).not.toBeNull()
  expect(region?.textContent).toBe("")
  await React.act(async () => {
    root.render(React.createElement(AttachmentReadingStatus, { reading: true }))
  })
  expect(container.querySelector("[aria-live]")).toBe(region)
  expect(region?.textContent).toBe("Reading files…")
  // It adds no chrome: it is only ever read out.
  expect(region?.className).toContain("sr-only")
})
