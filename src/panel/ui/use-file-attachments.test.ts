// @vitest-environment jsdom
/**
 * When a refusal is said, and for how long — the panel's own wiring, not either
 * half on its own.
 *
 * `attachment-notice.test.ts` has which notices there are as a plain function
 * and `attachment-notices.test.ts` has what they render as. This is the join:
 * the hook that holds the refusal and the component that shows it. Three things
 * are settled here and nowhere else, because all three are about time.
 *
 * It is said at the moment it happens. It is not revealed later, when something
 * unrelated finishes. And it stops being said when it stops being true — when
 * the draft it was refused against changes, and never above a conversation it
 * was not about.
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
  openConversation,
  removeFile,
  setActive,
  uploadChanged,
  useConversation,
  MAX_ATTACHMENT_BYTES,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { AttachmentNotices } from "./attachment-notices"
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

const huge = () => weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)

const retryUploads = vi.fn()

type Offer = (
  add: (files: readonly File[], conversationId?: string) => void,
  activeId: string,
) => void

/** The panel's own composition of the two pieces, and nothing else of the panel. */
function Surface({ offer }: { offer: Offer }) {
  const chat = useConversation()
  const attachments = useFileAttachments(chat, resources)
  return React.createElement(
    "div",
    null,
    React.createElement(
      "button",
      { type: "button", onClick: () => offer(attachments.addFiles, chat.active.id) },
      "Offer",
    ),
    React.createElement(AttachmentNotices, {
      refusal: attachments.refusal,
      files: attachments.files,
      imageInput: chat.active.remote?.capabilities.imageInput,
      onRetryUploads: retryUploads,
      onChooseFiles: vi.fn(),
      onDismissRefusal: attachments.clearRefusal,
    }),
  )
}

let container: HTMLDivElement
let root: Root

const button = (label: string) =>
  container.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)
/** Each notice's own live region, in the order they are read down the screen. */
const announced = () =>
  [...container.querySelectorAll("[aria-live]")].map((region) => region.textContent ?? "")

async function mount(offer: Offer) {
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

/** A draft file that has been through an upload and failed; the order is the store's. */
async function failUpload(store: ReturnType<typeof makeStore>) {
  await React.act(async () => {
    store.dispatch(attachFiles({ files: [attached], conversationId: "c0" }))
    store.dispatch(uploadChanged({ fileId: "f", to: "uploading" }))
    store.dispatch(uploadChanged({ fileId: "f", to: "failed", reason: "unavailable" }))
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

it("says a refusal at the moment it happens, without taking a failed upload's Retry away", async () => {
  const store = await mount((add) => add([huge()]))
  await failUpload(store)
  expect(announced()).toEqual([expect.stringContaining("Image didn't upload")])

  // The moment the panel used to lose one of the two, whichever way it ranked.
  await offerFiles()
  const [draft, refusal] = announced()
  expect(draft).toContain("Image didn't upload")
  expect(refusal).toContain("File is too large")
  expect(refusal).toContain("holiday.mp4")
  await React.act(async () => {
    button("Retry")!.click()
  })
  expect(retryUploads).toHaveBeenCalledExactlyOnceWith(["f"])
})

it("does not reveal a refusal later, when the upload beside it finishes", async () => {
  // The other half of the same bug. A refusal held back at the moment it
  // happened would surface when the retry succeeded, and read as though
  // something had gone wrong with the upload that had just gone right.
  const store = await mount((add) => add([huge()]))
  await failUpload(store)
  await offerFiles()
  expect(announced().join(" ")).toContain("File is too large")

  await React.act(async () => {
    store.dispatch(uploadChanged({ fileId: "f", to: "not-started" }))
    store.dispatch(uploadChanged({ fileId: "f", to: "uploading" }))
    store.dispatch(
      uploadChanged({
        fileId: "f",
        to: "stored",
        image: { digest: `sha256:${"ab".repeat(32)}`, mimeType: "image/png", size: 2 },
      }),
    )
  })
  // The upload's notice has gone because the upload is fine. The refusal is
  // where it already was, not newly arrived.
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
})

it("stops saying a refusal once the draft it was refused against has changed", async () => {
  // "Attach fewer, or send what is here first" is not something to go on saying
  // above a draft that no longer holds anything.
  const store = await mount((add) =>
    add(Array.from({ length: 21 }, (_, index) => weighing(`${index}.mp4`, 1))),
  )
  await React.act(async () => {
    store.dispatch(attachFiles({ files: [attached], conversationId: "c0" }))
  })
  await offerFiles()
  expect(announced()).toEqual([expect.stringContaining("Too many files")])
  // Not the old sentence, which listed all three bounds whichever one broke.
  expect(announced()[0]).not.toContain("64 MiB")

  await React.act(async () => {
    store.dispatch(removeFile("f"))
  })
  expect(announced()).toEqual([])
})

it("does not say one conversation's refusal above another's composer", async () => {
  // A folder walk finishes after somebody has moved on; the refusal belongs to
  // the draft it was dropped into, and quotes that draft's bounds.
  const store = await mount((add, activeId) => {
    expect(activeId).toBe("c1")
    add([huge()], "c0")
  })
  await React.act(async () => {
    store.dispatch(openConversation())
  })
  await offerFiles()
  expect(announced()).toEqual([])

  await React.act(async () => {
    store.dispatch(setActive("c0"))
  })
  expect(announced()).toEqual([expect.stringContaining("File is too large")])
})

it("refuses a file the + picker would refuse too, and does not send anybody to it", async () => {
  // The 720 MB .mp4 from the report, by way of the picker's own path: both
  // routes hold a file to the same bound, which is exactly why the advice to
  // try the other one was wrong.
  await mount((add) => add([huge()]))
  await offerFiles()
  expect(announced()[0]).toContain("64 MiB")
  expect(button("Choose files")).toBeNull()
  expect(button("Retry")).toBeNull()

  await React.act(async () => {
    button("Dismiss notification")!.click()
  })
  expect(announced()).toEqual([])
})
