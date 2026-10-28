import { AttachmentResource } from "../../conversation/testing"
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
const { readDroppedImage } = vi.hoisted(() => ({ readDroppedImage: vi.fn() }))
// This surface is the app's: composition asks the host whether there is a
// picker that can say where a file is, and here there is.
vi.mock("../../host", () => ({
  chooseAttachmentFiles,
  readAttachmentBytes,
  hasNativeHost: () => true,
  onSetupRecorded: () => Promise.resolve(() => {}),
  // No file here needs making ready, so the host never speaks. The tile it
  // would draw has its own tests in `use-file-attachments-readying`.
  onAttachmentReadying: () => Promise.resolve(() => {}),
  onAttachmentBatch: () => Promise.resolve(() => {}),
}))
vi.mock("../adapters/dropped-image", () => ({ readDroppedImage }))

import { createDependencies } from "../../composition/dependencies"
import {
  closeConversation,
  openConversation,
  scenarioEffects,
  setActive,
  useConversation,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  type ConversationEffects,
} from "../../conversation/testing"
import { makeStore } from "../../store"
import type { AttachmentResources } from "../adapters/attachment-resources"
import { useFileAttachments } from "./use-file-attachments"

let ticketCount = 0
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}
/** Only the two ways in that this exercises; nothing reads bytes here. */
const resources: AttachmentResources = {
  add: (files) =>
    files.map((file) =>
      AttachmentResource.create({
        name: file.name,
        mimeType: file.type || "application/octet-stream",
        size: file.size,
        previewUrl: "blob:test",
        path: null,
      }),
    ),
  addChosen: (chosen) =>
    chosen.map((file) =>
      AttachmentResource.create({
        name: file.name,
        mimeType: "application/octet-stream",
        size: file.size,
        previewUrl: "",
        path: file.path,
      }),
    ),
  canAdd: () => true,
  bytes: () => undefined,
  retain: () => {},
}

/** What the hook answered, kept where the test can drive it. */
let hook: ReturnType<typeof useFileAttachments>
let selectedResources = resources
/** Which conversation is showing, for the tests that close one. */
let chat: ReturnType<typeof useConversation>
function Surface() {
  chat = useConversation()
  hook = useFileAttachments(chat, selectedResources)
  return React.createElement("div")
}

let container: HTMLDivElement
let root: Root
let store: ReturnType<typeof makeStore>
const clickedInput = vi.fn()

beforeEach(async () => {
  ticketCount = 0
  selectedResources = resources
  chooseAttachmentFiles.mockReset()
  readAttachmentBytes.mockReset()
  readDroppedImage.mockReset()
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
  expect(draftFiles().map((file) => file.resource)).toMatchObject([
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

it("keeps a browser picker's original store through a delayed change", async () => {
  const storeA = store
  const conversationId = chat.active.id
  chooseAttachmentFiles.mockResolvedValue(null)
  const input = document.createElement("input")
  input.type = "file"
  input.click = clickedInput
  Object.defineProperty(hook.inputRef, "current", { value: input, writable: true })

  await act(async () => hook.chooseFiles())
  expect(clickedInput).toHaveBeenCalledOnce()
  await act(async () => hook.chooseFiles())
  expect(chooseAttachmentFiles).toHaveBeenCalledOnce()
  expect(clickedInput).toHaveBeenCalledOnce()
  expect(hook.refusal).toEqual({ reason: "picker-unavailable" })

  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  expect(storeB.getState().conversation.activeId).toBe(conversationId)
  store = storeB
  selectedResources = { ...resources }
  await act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store: storeB,
          children: React.createElement(Surface),
        }),
      ),
    )
  })

  await act(async () => hook.completeBrowserPicker([new File(["report"], "report.pdf")]))
  const draftIn = (target: typeof storeA) =>
    target
      .getState()
      .conversation.conversations.find(
        (conversation) => conversation.id === conversationId,
      )
      ?.draft.filter((part) => part.type === "file")
  expect(draftIn(storeA)).toHaveLength(1)
  expect(draftIn(storeA)?.[0]).toMatchObject({
    type: "file",
    resource: { name: "report.pdf" },
  })
  expect(draftIn(storeB)).toHaveLength(0)
})

it("releases a cancelled browser picker so the same input can be reopened", async () => {
  chooseAttachmentFiles.mockResolvedValue(null)
  const input = document.createElement("input")
  input.type = "file"
  input.click = clickedInput
  Object.defineProperty(hook.inputRef, "current", { value: input, writable: true })

  await act(async () => hook.chooseFiles())
  await act(async () => hook.cancelBrowserPicker())
  expect(clickedInput).toHaveBeenCalledOnce()

  await act(async () => hook.chooseFiles())
  expect(clickedInput).toHaveBeenCalledTimes(2)
  await act(async () => hook.completeBrowserPicker([new File(["report"], "report.pdf")]))
  expect(draftFiles()).toHaveLength(1)
})

it("ignores a browser picker change after teardown", async () => {
  chooseAttachmentFiles.mockResolvedValue(null)
  const input = document.createElement("input")
  input.type = "file"
  input.click = clickedInput
  Object.defineProperty(hook.inputRef, "current", { value: input, writable: true })
  const add = vi.spyOn(resources, "add")

  await act(async () => hook.chooseFiles())
  await act(async () => root.unmount())
  await act(async () => hook.completeBrowserPicker([new File(["late"], "late.pdf")]))

  expect(add).not.toHaveBeenCalled()
  expect(clickedInput).toHaveBeenCalledOnce()
  add.mockRestore()
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
  expect(image.resource).toMatchObject({ name: "holiday.heic", mimeType: "image/heic" })
  expect(image!.resource.previewUrl).not.toBe("")
  // The document was not read at all, and travels as the path.
  expect(readAttachmentBytes).not.toHaveBeenCalledWith("k-pdf")
  expect(picked.every((file) => file.path.startsWith("/"))).toBe(true)
  expect(document.resource).toMatchObject({
    name: "report.pdf",
    path: "/Users/ada/report.pdf",
    previewUrl: "",
  })
  expect(readAttachmentBytes).toHaveBeenCalledTimes(1)
  // Attachment order is the order they were picked in.
  expect(draftFiles().map((part) => part.resource.name)).toEqual([
    "holiday.heic",
    "report.pdf",
  ])
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
  expect(draftFiles().map((file) => file.resource)).toMatchObject([{ name: "huge.mp4" }])
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
  expect(files.map((part) => part.resource.name)).toEqual([
    "icon.ico",
    "photo.jpe",
    "art.tga",
  ])
  for (const part of files) {
    expect(part.resource.mimeType.startsWith("image/")).toBe(true)
    expect(part.resource.previewUrl).not.toBe("")
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
  expect(raw!.resource).toMatchObject({
    name: "holiday.cr3",
    mimeType: "image/x-canon-cr3",
  })
  expect(readAttachmentBytes).toHaveBeenCalledWith("k-cr3")
  // And the one the table does not know travels as its path, unread.
  expect(document.resource).toMatchObject({
    name: "report.pdf",
    path: "/Users/ada/report.pdf",
  })
  expect(readAttachmentBytes).not.toHaveBeenCalledWith("k-pdf")
})

/**
 * A draft closed while its selection was still being described.
 *
 * Both file types, because the two take different routes to the same sentence:
 * a path never reads a byte, an image reads them all first. An earlier version
 * of this test used a PDF alone and claimed "the bytes were never spent" — true
 * of the path it happened to pick and false of the branch beside it.
 */
const chosenFile = (name: string, mimeType: string) => ({
  path: `/Users/ada/${name}`,
  name,
  size: 4,
  mimeType,
  ticket: `k${ticketCount++}`,
})

it("says so when the draft a slow selection was for has been closed", async () => {
  // A placeholder can be forty-five seconds behind the gesture, and a tab can
  // be closed in forty-five seconds.
  for (const file of [
    chosenFile("report.pdf", "application/pdf"),
    chosenFile("shot.png", "image/png"),
  ]) {
    readAttachmentBytes.mockClear()
    const going = chat.active.id
    await act(async () => {
      store.dispatch(closeConversation(going))
    })
    expect(chat.active.id).not.toBe(going)

    await act(async () => {
      await hook.addChosenFiles([file], going)
    })

    // Said where the person is looking, because the conversation it is about
    // is gone and saying it there would be saying it to nobody. Said at all
    // because the alternative is the silence this module refuses everywhere
    // else: files chosen, none attached, and not a word about any of them. It
    // used to return here without one.
    expect(hook.refusal, file.name).toEqual({ reason: "conversation-closed" })
    expect(draftFiles(), file.name).toHaveLength(0)
    // And the draft was checked before the read rather than after it, so the
    // image's bytes were never fetched either.
    expect(readAttachmentBytes, file.name).not.toHaveBeenCalled()

    await act(async () => {
      store.dispatch(openConversation())
    })
  }
})

it("says the same thing when the draft goes while its images are being read", async () => {
  // The other half, and the one the early check cannot cover: closing happens
  // *during* the read. Those bytes are spent and thrown away — there is no
  // version of this where they are not, short of cancelling a read in flight —
  // so the promise is the sentence, not the saving.
  const going = chat.active.id
  readAttachmentBytes.mockImplementation(async () => {
    store.dispatch(closeConversation(going))
    return new ArrayBuffer(4)
  })

  await act(async () => {
    await hook.addChosenFiles([chosenFile("shot.png", "image/png")], going)
  })

  expect(readAttachmentBytes).toHaveBeenCalledTimes(1)
  expect(hook.refusal).toEqual({ reason: "conversation-closed" })
  expect(draftFiles()).toHaveLength(0)
})

it("does not route a closed picker owner's preflight refusal into a replacement provider", async () => {
  const storeA = store
  const conversationId = chat.active.id
  const selection = deferred<unknown[]>()
  chooseAttachmentFiles.mockReturnValue(selection.promise)

  await act(async () => hook.chooseFiles())
  expect(chooseAttachmentFiles).toHaveBeenCalledOnce()

  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  expect(storeB.getState().conversation.activeId).toBe(conversationId)
  const resourcesB = { ...resources }
  store = storeB
  selectedResources = resourcesB
  await act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store: storeB,
          children: React.createElement(Surface),
        }),
      ),
    )
  })
  expect(hook.captureDestination().store).toBe(storeB)
  await act(async () => storeA.dispatch(closeConversation(conversationId)))

  await act(async () => {
    selection.resolve([
      {
        path: "/late.pdf",
        name: "late.pdf",
        size: 1,
        mimeType: "application/pdf",
        ticket: "late-pdf",
      },
    ])
    await new Promise((resolve) => setTimeout(resolve, 0))
  })

  expect(hook.refusal).toBeNull()
  expect(readAttachmentBytes).not.toHaveBeenCalled()
  expect(
    storeB
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(0)
})

it("does not route a closed read owner's refusal into a replacement provider", async () => {
  const storeA = store
  const conversationId = chat.active.id
  const imageRead = deferred<ArrayBuffer>()
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/late.png",
      name: "late.png",
      size: 1,
      mimeType: "image/png",
      ticket: "late-image",
    },
  ])
  readAttachmentBytes.mockReturnValue(imageRead.promise)

  await act(async () => hook.chooseFiles())
  await act(async () => {
    await vi.waitFor(() => expect(readAttachmentBytes).toHaveBeenCalledWith("late-image"))
  })

  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  expect(storeB.getState().conversation.activeId).toBe(conversationId)
  const resourcesB = { ...resources }
  store = storeB
  selectedResources = resourcesB
  await act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store: storeB,
          children: React.createElement(Surface),
        }),
      ),
    )
  })
  expect(hook.captureDestination().store).toBe(storeB)
  await act(async () => storeA.dispatch(closeConversation(conversationId)))

  await act(async () => {
    imageRead.resolve(new ArrayBuffer(4))
    await new Promise((resolve) => setTimeout(resolve, 0))
  })

  expect(hook.refusal).toBeNull()
  expect(
    storeB
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(0)
})

it("refuses a file linked by path for a conversation that runs on an SSH host, and keeps taking images", async () => {
  await act(async () => {
    chat.setSelection(chat.active.id, {
      agent: "claude",
      model: "sonnet",
      approvalMode: "ask",
      environment: "devbox",
    })
  })
  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/notes.md",
      name: "notes.md",
      size: 10,
      mimeType: "text/markdown",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  // The host has files of its own: this path would name nothing there, or
  // something else. Nothing is attached, and the person is told why.
  expect(draftFiles()).toHaveLength(0)
  expect(hook.refusal).toEqual({ reason: "file-not-on-host", name: "notes.md" })

  chooseAttachmentFiles.mockResolvedValue([
    {
      path: "/Users/ada/shot.png",
      name: "shot.png",
      size: 4,
      mimeType: "image/png",
      ticket: `k${ticketCount++}`,
    },
  ])
  await act(async () => {
    await hook.chooseFiles()
  })
  expect(draftFiles().map((file) => file.resource)).toMatchObject([{ name: "shot.png" }])
  expect(hook.refusal).toBeNull()
})

it("keeps refusals with the actual store when the mounted provider changes", async () => {
  const refuseFromA = hook.refuse
  const destinationA = hook.captureDestination()
  expect(destinationA.store).toBe(store)
  await act(async () => hook.refuse({ reason: "too-many-files" }))
  expect(hook.refusal).toEqual({ reason: "too-many-files" })

  const nextStore = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => nextStore.dispatch(openConversation()))
  expect(nextStore.getState().conversation.activeId).toBe(
    store.getState().conversation.activeId,
  )
  store = nextStore
  const nextResources = { ...resources }
  selectedResources = nextResources
  await act(async () => {
    root.render(
      React.createElement(Provider, { store, children: React.createElement(Surface) }),
    )
  })
  expect(hook.refusal).toBeNull()

  await act(async () => hook.refuse({ reason: "file-too-large", files: [] }))
  const refusalB = hook.refusal
  await act(async () => refuseFromA({ reason: "too-many-files" }))
  expect(hook.refusal).toEqual(refusalB)
})

it("keeps delayed refusals by open conversation and answers only that conversation", async () => {
  const firstId = chat.active.id
  const firstDestination = hook.captureDestination()
  await act(async () => store.dispatch(openConversation()))
  const secondId = chat.active.id
  const secondDestination = hook.captureDestination()
  expect(secondId).not.toBe(firstId)

  // This result belongs to the still-open first tab, even though the second
  // tab is active when it arrives.
  await act(async () => hook.refuse({ reason: "too-many-files" }, firstDestination))
  expect(hook.refusal).toBeNull()
  await act(async () =>
    hook.refuse({ reason: "file-too-large", files: [] }, secondDestination),
  )
  expect(hook.refusal).toEqual({ reason: "file-too-large", files: [] })

  await act(async () => store.dispatch(setActive(firstId)))
  expect(hook.refusal).toEqual({ reason: "too-many-files" })
  await act(async () => store.dispatch(setActive(secondId)))
  expect(hook.refusal).toEqual({ reason: "file-too-large", files: [] })

  await act(async () => hook.draftSent(firstId))
  expect(hook.refusal).toEqual({ reason: "file-too-large", files: [] })
  await act(async () => store.dispatch(setActive(firstId)))
  expect(hook.refusal).toBeNull()

  // Closing a tab prunes its still-unanswered notice without disturbing B.
  await act(async () => hook.refuse({ reason: "too-many-files" }, firstDestination))
  expect(hook.refusal).toEqual({ reason: "too-many-files" })
  await act(async () => store.dispatch(closeConversation(firstId)))
  expect(chat.active.id).toBe(secondId)
  expect(hook.refusal).toEqual({ reason: "file-too-large", files: [] })
})

it("keeps an older image read in its store and its finally out of the newer operation", async () => {
  const firstRead = deferred<ArrayBuffer>()
  const secondRead = deferred<ArrayBuffer>()
  const storeA = store
  const conversationId = chat.active.id
  chooseAttachmentFiles
    .mockResolvedValueOnce([
      { path: "/a.png", name: "a.png", size: 1, mimeType: "image/png", ticket: "a" },
    ])
    .mockResolvedValueOnce([
      { path: "/b.png", name: "b.png", size: 1, mimeType: "image/png", ticket: "b" },
    ])
  readAttachmentBytes
    .mockReturnValueOnce(firstRead.promise)
    .mockReturnValueOnce(secondRead.promise)
  await act(async () => hook.chooseFiles())
  await act(async () => {
    await vi.waitFor(() => expect(readAttachmentBytes).toHaveBeenCalledWith("a"))
  })

  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  expect(storeB.getState().conversation.activeId).toBe(conversationId)
  const resourcesB = { ...resources }
  store = storeB
  selectedResources = resourcesB
  await act(async () => {
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store: storeB,
          children: React.createElement(Surface),
        }),
      ),
    )
  })
  await act(async () => hook.chooseFiles())
  await act(async () => {
    await vi.waitFor(() => expect(readAttachmentBytes).toHaveBeenCalledWith("b"))
  })
  expect(hook.isPending(conversationId)).toBe(true)

  await act(async () => firstRead.resolve(new ArrayBuffer(4)))
  expect(
    storeA
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(1)
  expect(
    storeB
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(0)
  expect(hook.isPending(conversationId)).toBe(true)
  expect(hook.pendingFiles.map((file) => file.name)).toEqual(["b.png"])
  await act(async () => secondRead.resolve(new ArrayBuffer(4)))
  expect(
    storeB
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(1)
  expect(hook.isPending(conversationId)).toBe(false)
})

it("refuses an active picker ID collision without replacing its destination", async () => {
  const first = "01234567-89ab-cdef-0123-456789abcdef"
  const randomUUID = vi
    .spyOn(crypto, "randomUUID")
    .mockReturnValueOnce(first)
    .mockReturnValueOnce(first)
    .mockReturnValueOnce(first)
  const answers = new Map<string, (files: unknown[]) => void>()
  chooseAttachmentFiles.mockImplementation(
    (requestId: string) => new Promise((resolve) => answers.set(requestId, resolve)),
  )

  await act(async () => hook.chooseFiles())
  await act(async () => hook.chooseFiles())
  expect(chooseAttachmentFiles).toHaveBeenCalledTimes(1)
  expect(hook.refusal).toEqual({ reason: "picker-unavailable" })

  await act(async () => {
    answers.get(`p-${first}`)?.([])
    await Promise.resolve()
  })
  await act(async () => hook.chooseFiles())
  expect(chooseAttachmentFiles.mock.calls.map(([requestId]) => requestId)).toEqual([
    `p-${first}`,
    `p-${first}`,
  ])

  await act(async () => {
    answers.get(`p-${first}`)?.([])
    await Promise.resolve()
  })
  randomUUID.mockRestore()
})

it("does not start another native read when an open picker returns after teardown", async () => {
  const chosen = deferred<unknown[]>()
  const cancelled = deferred<null>()
  const click = vi.fn()
  const input = document.createElement("input")
  input.click = click
  Object.defineProperty(hook.inputRef, "current", { value: input, writable: true })
  const addChosen = vi.spyOn(resources, "addChosen")
  chooseAttachmentFiles
    .mockReturnValueOnce(chosen.promise)
    .mockReturnValueOnce(cancelled.promise)

  await act(async () => hook.chooseFiles())
  await act(async () => hook.chooseFiles())
  expect(chooseAttachmentFiles).toHaveBeenCalledTimes(2)
  await act(async () => root.unmount())

  await act(async () => {
    chosen.resolve([
      {
        path: "/late.png",
        name: "late.png",
        size: 1,
        mimeType: "image/png",
        ticket: "late",
      },
    ])
    cancelled.resolve(null)
    await chosen.promise
    await cancelled.promise
    await Promise.resolve()
  })

  expect(click).not.toHaveBeenCalled()
  expect(readAttachmentBytes).not.toHaveBeenCalled()
  expect(addChosen).not.toHaveBeenCalled()
  await hook.addImageUrl("https://example.com/late.png")
  await hook.addChosenFiles([
    {
      path: "/late.png",
      name: "late.png",
      size: 1,
      mimeType: "image/png",
      ticket: "late",
    },
  ])
  expect(readDroppedImage).not.toHaveBeenCalled()
  expect(readAttachmentBytes).not.toHaveBeenCalled()
  addChosen.mockRestore()
})
