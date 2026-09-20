// @vitest-environment jsdom
/**
 * What the composer ends up saying when a refusal and the draft's own files are
 * both true at once — the panel's own wiring, not either half on its own.
 *
 * `attachment-notice.test.ts` has the order as a plain function and
 * `attachment-notification.test.ts` has what one notice renders as. This is the
 * join: the hook that holds the refusal, the function that ranks it against the
 * draft, and the notification that is actually on screen. It is where the defect
 * lived — a refusal used to be rendered as its own red paragraph and to gate the
 * notification out entirely, so a limits line left over from a drop took a
 * failed upload's Retry off the screen with it.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The conversation barrel also exports its components, which need the whole UI
// package resolved. Its `testing` entry is the same pure functions with no
// component among them, so the barrel is mocked with that: one definition.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { createDependencies } from "../../composition/dependencies"
import {
  attachFiles,
  isImageFile,
  removeFile,
  uploadChanged,
  useConversation,
  MAX_ATTACHMENT_BYTES,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { attachmentNotice } from "../application/attachment-notice"
import { AttachmentNotification } from "./attachment-notification"
import { useFileAttachments } from "./use-file-attachments"

const attached = {
  type: "file" as const,
  id: "f",
  name: "photo.png",
  mimeType: "image/png",
  size: 1,
  previewUrl: "blob:test-file",
  upload: { status: "not-started" as const },
}

/**
 * The resource store, as much of it as a refusal needs. jsdom has no
 * `createObjectURL`, and none of these tests gets as far as wanting one: every
 * one of them is about something this panel would not take.
 */
const resources: AttachmentResources = {
  add: () => {
    throw new Error("no test here attaches bytes")
  },
  canAdd: () => true,
  bytes: () => undefined,
  retain: () => {},
}

/** A File of a given weight without allocating it: only `size` and `name` are read. */
const weighing = (name: string, size: number) =>
  ({ name, size, type: "video/mp4" }) as File

const retryUploads = vi.fn()

/** The panel's own composition of the three pieces, and nothing else of the panel. */
function Surface({ offer }: { offer: (add: (files: readonly File[]) => void) => void }) {
  const chat = useConversation()
  const attachments = useFileAttachments(chat, resources)
  const notice = attachmentNotice({
    refusal: attachments.refusal,
    files: attachments.files.map((file) => ({
      id: file.id,
      name: file.name,
      image: isImageFile(file.mimeType),
      upload: file.upload,
    })),
    imageInput: chat.active.remote?.capabilities.imageInput,
  })
  return React.createElement(
    "div",
    null,
    React.createElement(
      "button",
      { type: "button", onClick: () => offer(attachments.addFiles) },
      "Offer",
    ),
    notice
      ? React.createElement(AttachmentNotification, {
          notice,
          onRetryUploads: retryUploads,
          onChooseFiles: vi.fn(),
          onDismiss: attachments.clearRefusal,
        })
      : null,
  )
}

let container: HTMLDivElement
let root: Root

const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)
const announced = () => container.querySelector("[aria-live]")?.textContent ?? ""

async function mount(offer: (add: (files: readonly File[]) => void) => void) {
  const store = makeStore(createDependencies({}))
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface, { offer }),
      }),
    )
  })
  return store
}

/** Press the button that hands `addFiles` whatever this test is offering. */
async function offerFiles() {
  await React.act(async () => {
    container.querySelector<HTMLButtonElement>("button")!.click()
  })
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom has no `matchMedia`, and the notification asks it about reduced
  // motion before it plays its exit.
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

it("keeps a failed upload's notice and its Retry while a refusal is set", async () => {
  const huge = weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)
  const store = await mount((add) => add([huge]))
  await React.act(async () => {
    store.dispatch(attachFiles({ files: [attached], conversationId: "c0" }))
    // An upload only fails from in flight; the conversation owns that order.
    store.dispatch(uploadChanged({ fileId: "f", to: "uploading" }))
    store.dispatch(uploadChanged({ fileId: "f", to: "failed", reason: "unavailable" }))
  })
  expect(announced()).toContain("Image didn't upload")
  const retry = button("Retry")
  expect(retry).not.toBeNull()

  // Now refuse something. This is the moment the old panel lost the Retry.
  await offerFiles()
  expect(announced()).toContain("Image didn't upload")
  expect(button("Retry")).not.toBeNull()
  await React.act(async () => {
    button("Retry")!.click()
  })
  expect(retryUploads).toHaveBeenCalledExactlyOnceWith(["f"])

  // And the refusal was really set all along rather than never having happened:
  // with the failed file gone from the draft, it is what the notice says.
  await React.act(async () => {
    store.dispatch(removeFile("f"))
  })
  expect(announced()).toContain("File is too large")
  expect(announced()).toContain("holiday.mp4")
})

it("says a dropped file was too heavy without sending anybody to the picker", async () => {
  // The 720 MB .mp4 from the report, by way of the + picker's own path: both
  // routes hold a file to the same bound, which is exactly why the advice to
  // try the other one was wrong.
  await mount((add) => add([weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)]))
  await offerFiles()
  expect(announced()).toContain("File is too large")
  expect(announced()).toContain("holiday.mp4")
  expect(announced()).toContain("64 MiB")
  expect(button("Choose files")).toBeNull()
  expect(button("Retry")).toBeNull()
})

it("counts files and weighs the draft separately, and lets a refusal be put away", async () => {
  const many = Array.from({ length: 21 }, (_, index) => weighing(`${index}.mp4`, 1))
  await mount((add) => add(many))
  await offerFiles()
  expect(announced()).toContain("Too many files")
  // Not the old sentence, which listed all three bounds whichever one was broken.
  expect(announced()).not.toContain("64 MiB")
  await React.act(async () => {
    button("Dismiss notification")!.click()
  })
  expect(container.querySelector("[aria-live]")).toBeNull()
})
