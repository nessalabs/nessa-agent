// @vitest-environment jsdom
/**
 * Where the composer's `+` gets its files from, and what happens to a draft
 * when it is the host that answers.
 *
 * This is the one thing the rest of the attachment tests cannot reach: they
 * run with no host, where the picker returns null and the page's own file
 * input is used. Here the seam is substituted, so both halves are exercised —
 * a desktop host that hands over real paths, and a browser that has none.
 *
 * The bounds are the point of the first test. Every byte bound this composer
 * has exists because the window would otherwise be holding the bytes; a path
 * holds nothing, so the 700 MB video from the original report is an ordinary
 * attachment here and the count bound is all that is left.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { act } from "react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("../../conversation", () => import("../../conversation/testing"))

// Hoisted with the mock, because the factory runs before the module body.
const { chooseAttachmentFiles, readAttachmentBytes } = vi.hoisted(() => ({
  chooseAttachmentFiles: vi.fn(),
  readAttachmentBytes: vi.fn(),
}))
// This surface is the app's: composition asks the host whether there is a
// picker that can say where a file is, and here there is.
vi.mock("../../host", () => ({
  chooseAttachmentFiles,
  readAttachmentBytes,
  hasNativeHost: () => true,
  // No file here needs making ready, so the host never speaks. The tile it
  // would draw has its own tests in `use-file-attachments-readying`.
  onAttachmentReadying: () => Promise.resolve(() => {}),
  onAttachmentBatch: () => Promise.resolve(() => {}),
}))

import { createDependencies } from "../../composition/dependencies"
import {
  closeConversation,
  openConversation,
  scenarioEffects,
  useConversation,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type ConversationEffects,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { useFileAttachments } from "./use-file-attachments"

let taken = 0
let ticketCount = 0
/** Only the two ways in that this exercises; nothing reads bytes here. */
const resources: AttachmentResources = {
  add: (files) =>
    files.map((file) => ({
      type: "file" as const,
      id: `a${taken++}`,
      name: file.name,
      mimeType: file.type || "application/octet-stream",
      size: file.size,
      previewUrl: "blob:test",
      upload: { status: "not-started" as const },
      path: null,
    })),
  addChosen: (chosen) =>
    chosen.map((file) => ({
      type: "file" as const,
      id: `c${taken++}`,
      name: file.name,
      mimeType: "application/octet-stream",
      size: file.size,
      previewUrl: "",
      upload: { status: "not-started" as const },
      path: file.path,
    })),
  canAdd: () => true,
  bytes: () => undefined,
  retain: () => {},
}

/** What the hook answered, kept where the test can drive it. */
let hook: ReturnType<typeof useFileAttachments>
/** Which conversation is showing, for the tests that close one. */
let chat: ReturnType<typeof useConversation>
function Surface() {
  chat = useConversation()
  hook = useFileAttachments(chat, resources)
  return React.createElement("div")
}

let container: HTMLDivElement
let root: Root
let store: ReturnType<typeof makeStore>
const clickedInput = vi.fn()

beforeEach(async () => {
  taken = 0
  ticketCount = 0
  chooseAttachmentFiles.mockReset()
  readAttachmentBytes.mockReset()
  readAttachmentBytes.mockResolvedValue(new ArrayBuffer(4))
  clickedInput.mockReset()
  container = document.createElement("div")
  document.body.append(container)
  store = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  root = createRoot(container)
  await act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store,
          children: React.createElement(Surface),
        }),
      ),
    )
  })
  await act(async () => {
    store.dispatch(openConversation())
  })
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

const draftFiles = () =>
  store
    .getState()
    .conversation.conversations.find(
      (conversation) => conversation.id === store.getState().conversation.activeId,
    )!
    .draft.filter((part) => part.type === "file")

it("attaches a file far past every byte bound, because a path weighs nothing", async () => {
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/holiday.mp4",
      name: "holiday.mp4",
      size: 700 * 1024 * 1024,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toMatchObject([
    { name: "holiday.mp4", path: "/Users/ada/holiday.mp4" },
  ])
  expect(hook.refusal).toBeNull()
})

it("keeps the one bound that is about the draft rather than about bytes", async () => {
  const chosen = Array.from({ length: MAX_DRAFT_ATTACHMENTS + 1 }, (_, index) => ({
    path: `/Users/ada/${index}.pdf`,
    name: `${index}.pdf`,
    size: 1,
    mimeType: "application/pdf",
    ticket: `k${index}`,
  }))
  chooseAttachmentFiles.mockResolvedValue(chosen)
  await act(async () => {
    await hook.chooseFiles()
  })
  // Refused whole: a draft one file short of what was chosen is not what
  // anybody asked for.
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toEqual({ reason: "too-many-files" })

  chooseAttachmentFiles.mockResolvedValue(chosen.slice(0, MAX_DRAFT_ATTACHMENTS))
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(MAX_DRAFT_ATTACHMENTS)
  // Attaching answers the refusal it replaced.
  expect(hook.refusal).toBeNull()
})

it("cancelling the picker changes nothing and says nothing", async () => {
  chooseAttachmentFiles.mockResolvedValue([])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toBeNull()
})

it("falls back to the page's own file input when there is no host to ask", async () => {
  // Outside the desktop app: the seam answers null, and the browser's input is
  // used instead. It reads bytes, so images still work and nothing else can go.
  chooseAttachmentFiles.mockResolvedValue(null)
  const input = document.createElement("input")
  input.type = "file"
  input.click = clickedInput
  Object.defineProperty(hook.inputRef, "current", { value: input, writable: true })
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(clickedInput).toHaveBeenCalledOnce()
  expect(draftFiles()).toHaveLength(0)
})

it("says why the host would not hand a file over, rather than attaching fewer", async () => {
  chooseAttachmentFiles.mockRejectedValue({
    reason: "path-not-text",
    shown: "rep?rt.pdf",
    detail: null,
  })
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toEqual({ reason: "file-not-nameable", name: "rep?rt.pdf" })

  // A failure with no typed reason is the picker not working, and is still said.
  chooseAttachmentFiles.mockRejectedValue(new Error("no such command"))
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(hook.refusal).toEqual({ reason: "picker-unavailable" })
})

it("uploads a picked image and points at a picked file, from one selection", async () => {
  // The rule the whole feature turns on: the type decides, not the gesture. One
  // pick, two routes, and the picker's images are read so they can be uploaded
  // exactly as a dropped image is.
  const picked = [
    {
      path: "/Users/ada/holiday.heic",
      name: "holiday.heic",
      size: 9,
      mimeType: "image/heic",
      ticket: "k-heic",
    },
    {
      path: "/Users/ada/report.pdf",
      name: "report.pdf",
      size: 700 * 1024 * 1024,
      mimeType: "application/pdf",
      ticket: "k-pdf",
    },
  ]
  chooseAttachmentFiles.mockResolvedValue(picked)
  await act(async () => {
    await hook.chooseFiles()
  })

  const [image, document] = draftFiles()
  // The image was read, typed from its name, and has bytes to upload.
  // Read by its ticket, never by its path: the host resolves what it minted.
  expect(readAttachmentBytes).toHaveBeenCalledWith("k-heic")
  expect(image).toMatchObject({ name: "holiday.heic", mimeType: "image/heic" })
  expect(image!.previewUrl).not.toBe("")
  // The document was not read at all, and travels as the path.
  expect(readAttachmentBytes).not.toHaveBeenCalledWith("k-pdf")
  expect(picked.every((file) => file.path.startsWith("/"))).toBe(true)
  expect(document).toMatchObject({
    name: "report.pdf",
    path: "/Users/ada/report.pdf",
    previewUrl: "",
  })
  expect(readAttachmentBytes).toHaveBeenCalledTimes(1)
  // Attachment order is the order they were picked in.
  expect(draftFiles().map((part) => part.name)).toEqual(["holiday.heic", "report.pdf"])
})

it("holds a picked image to the byte bound, and a picked file to none", async () => {
  // Both byte bounds exist because the bytes would be held here, so the image
  // is held to one and the document to none. The host refuses to read more
  // than the panel would keep, and that refusal is said in the composer's own
  // words — the same ones a drop of the same image gets, because it is the
  // same bound reached by another route.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/huge.png",
      name: "huge.png",
      size: MAX_ATTACHMENT_BYTES + 1,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  readAttachmentBytes.mockRejectedValue({
    reason: "file-too-large",
    shown: "huge.png",
    detail: "over what the panel will hold",
  })
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(0)
  // No platform type beside it: the host refused to read the file, so there
  // is none to carry — and no picker left to advise either, since this file
  // came from one.
  expect(hook.refusal).toEqual({
    reason: "file-too-large",
    files: [{ name: "huge.png", type: "" }],
  })

  // The same weight as a path is nobody's problem: nothing is read at all.
  readAttachmentBytes.mockClear()
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/huge.mp4",
      name: "huge.mp4",
      size: MAX_ATTACHMENT_BYTES * 12,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toMatchObject([{ name: "huge.mp4" }])
  expect(readAttachmentBytes).not.toHaveBeenCalled()
})

it("refuses a whole selection when one of its images will not read", async () => {
  // Attaching some of what was picked is the silent half-failure this design
  // exists to avoid, so the reason is said and nothing lands.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/a.png",
      name: "a.png",
      size: 4,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
    {
      path: "/Users/ada/report.pdf",
      name: "report.pdf",
      size: 4,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  readAttachmentBytes.mockRejectedValue({
    reason: "file-unreadable",
    shown: "a.png",
    detail: null,
  })
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toEqual({ reason: "file-unreadable", name: "a.png" })
})

it("refuses a path the gateway would refuse, while the file can still be swapped", async () => {
  // Refused at attach rather than at send: a `..` component does not survive
  // the path being written as a URI, so the link would name a different file.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/../../etc/report.pdf",
      name: "report.pdf",
      size: 4,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toEqual({ reason: "file-not-linkable", name: "report.pdf" })
  expect(readAttachmentBytes).not.toHaveBeenCalled()

  // And a bracketed folder is not one of those. It used to be, on the grounds
  // that a bracket could close the markdown link the agent is handed the path
  // in; the adapter encodes that link now, so this is an ordinary directory.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/[drafts]/report.pdf",
      name: "report.pdf",
      size: 4,
      mimeType: "",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles()).toHaveLength(1)
  expect(hook.refusal).toBe(null)
})

it("routes an image the platform knows and the extension table does not, like a drop", async () => {
  // The defect a corpus of `.png` and `.heic` could never show. These are
  // image formats the platform recognises and this app's extension table has
  // never heard of, so classifying a picked file from the table alone sent
  // them down the path route while the same file dropped was uploaded — one
  // file, two routes, a per-read approval on one of them and a different bound.
  //
  // They agree now because the picker reports the platform's type and it goes
  // exactly where a dropped file's `type` goes.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/icon.ico",
      name: "icon.ico",
      size: 4,
      mimeType: "image/vnd.microsoft.icon",
      ticket: "k-ico",
    },
    {
      path: "/Users/ada/photo.jpe",
      name: "photo.jpe",
      size: 4,
      mimeType: "image/jpeg",
      ticket: "k-jpe",
    },
    {
      path: "/Users/ada/art.tga",
      name: "art.tga",
      size: 4,
      mimeType: "image/x-tga",
      ticket: "k-tga",
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })

  // Every one of them was read and staged as an image, and none travels as a
  // path: `path` on a draft file is what makes it a linked file.
  const files = draftFiles()
  expect(files.map((part) => part.name)).toEqual(["icon.ico", "photo.jpe", "art.tga"])
  for (const part of files) {
    expect(part.mimeType.startsWith("image/")).toBe(true)
    expect(part.previewUrl).not.toBe("")
  }
  expect(readAttachmentBytes).toHaveBeenCalledTimes(3)
  // Read by ticket, never by path.
  expect(readAttachmentBytes.mock.calls.map((call) => call[0])).toEqual([
    "k-ico",
    "k-jpe",
    "k-tga",
  ])
})

it("still falls back to the extension when the platform has no answer", async () => {
  // Windows always, and macOS for a type it knows and has no MIME name for.
  // The table is what catches a RAW file then, which is the case it exists for.
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/holiday.cr3",
      name: "holiday.cr3",
      size: 4,
      mimeType: "",
      ticket: "k-cr3",
    },
    {
      path: "/Users/ada/report.pdf",
      name: "report.pdf",
      size: 4,
      mimeType: "",
      ticket: "k-pdf",
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  const [raw, document] = draftFiles()
  expect(raw).toMatchObject({ name: "holiday.cr3", mimeType: "image/x-canon-cr3" })
  expect(readAttachmentBytes).toHaveBeenCalledWith("k-cr3")
  // And the one the table does not know travels as its path, unread.
  expect(document).toMatchObject({ name: "report.pdf", path: "/Users/ada/report.pdf" })
  expect(readAttachmentBytes).not.toHaveBeenCalledWith("k-pdf")
})

it("says so when the draft a slow selection was for has been closed", async () => {
  // A placeholder can be forty-five seconds behind the gesture, and a tab can
  // be closed in forty-five seconds. The files arrive perfectly well and there
  // is nothing left to put them on.
  const going = chat.active.id
  const chosen = [
    {
      path: "/Users/ada/report.pdf",
      name: "report.pdf",
      size: 4,
      mimeType: "application/pdf",
      ticket: `k${ticketCount++}`,
    },
  ]
  await act(async () => {
    store.dispatch(closeConversation(going))
  })
  expect(chat.active.id).not.toBe(going)

  await act(async () => {
    await hook.addChosenFiles(chosen, going)
  })

  // Said where the person is looking, because the conversation it is about is
  // gone and saying it there would be saying it to nobody. Said at all because
  // the alternative is the silence this module refuses everywhere else: three
  // files chosen, none attached, and not a word about any of them. It used to
  // return here without one.
  expect(hook.refusal).toEqual({ reason: "conversation-closed" })
  expect(draftFiles()).toHaveLength(0)
  // And the bytes were never spent: the draft was gone before the upload.
  expect(readAttachmentBytes).not.toHaveBeenCalled()
})
