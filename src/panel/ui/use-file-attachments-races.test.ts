// @vitest-environment jsdom
/**
 * Controlled reproduction against the actual App, shared Markdown editor and
 * Redux submission path. Only outside host/fetch effects are substituted.
 * Native reads and explicit image fetches stay pending until the test settles
 * them, so submission is checked before and after the same read.
 */
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { flushSync } from "react-dom"
import { Provider } from "react-redux"
import { beforeEach, afterEach, expect, it, vi } from "vitest"
import type { DroppedOnPanel } from "../../host"

const hostBoundary = vi.hoisted(() => ({
  choose: vi.fn(),
  read: vi.fn(),
  drop: undefined as undefined | ((payload: DroppedOnPanel) => void),
  batch: undefined as
    undefined | ((batch: string, gesture: "picked" | "dropped") => void),
}))

const folderBoundary = vi.hoisted(() => ({ read: vi.fn() }))

vi.mock("../adapters/dropped-folder", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../adapters/dropped-folder")>()),
  readDroppedFolder: folderBoundary.read,
}))

vi.mock("../../host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../host")>()),
  hasNativeHost: () => true,
  chooseAttachmentFiles: hostBoundary.choose,
  readAttachmentBytes: hostBoundary.read,
  onAttachmentReadying: () => Promise.resolve(() => {}),
  onAttachmentBatch: (
    handler: (batch: string, gesture: "picked" | "dropped") => void,
  ) => {
    hostBoundary.batch = handler
    return Promise.resolve(() => {})
  },
  onAttachmentDragging: () => Promise.resolve(() => {}),
  onAttachmentDropped: (handler: (payload: DroppedOnPanel) => void) => {
    hostBoundary.drop = handler
    return Promise.resolve(() => {})
  },
}))

import { App } from "./app"
import { ClockProvider } from "../../clock"
import { createAttachmentResources } from "../adapters/attachment-resources"
import { makeStore, type AppStore } from "../../store"
import {
  attachFiles,
  closeConversation,
  openConversation,
  scenarioEffects,
  setActive,
  setDraft,
} from "../../conversation/testing"
import { sessionReady } from "../../session/testing"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((answer, refuse) => {
    resolve = answer
    reject = refuse
  })
  return { promise, resolve, reject }
}

let container: HTMLDivElement
let root: Root
let store: AppStore
let resources: ReturnType<typeof createAttachmentResources>
let send: ReturnType<typeof vi.fn>
let cleanups: (() => void)[]
let prototypeProperties: [object, string, PropertyDescriptor | undefined][]

function supplyProperty(target: object, name: string, value: unknown) {
  prototypeProperties.push([target, name, Object.getOwnPropertyDescriptor(target, name)])
  Object.defineProperty(target, name, { configurable: true, value })
}

async function mount(text = "", targetStore = store, targetResources = resources) {
  if (text) targetStore.dispatch(setDraft({ draft: [{ type: "text", text }] }))
  await renderApp(targetStore, targetResources)
  // The editor is made asynchronously, and focuses itself when it attaches
  // (`use-composer.ts`). Wait for that before anything is clicked: under load
  // it could otherwise land after a test opened the tray, whose blur rule
  // then closes it (#396).
  await act(async () => {
    await vi.waitFor(() => {
      const editor = container.querySelector('[contenteditable="true"]')
      expect(editor, "the real Markdown editor must be mounted").not.toBeNull()
      expect(document.activeElement).toBe(editor)
    })
  })
}

async function renderApp(targetStore = store, targetResources = resources) {
  await act(async () => root.render(appElement(targetStore, targetResources)))
}

function renderAppSynchronously(targetStore = store, targetResources = resources) {
  flushSync(() => root.render(appElement(targetStore, targetResources)))
}

function appElement(targetStore: AppStore, targetResources: typeof resources) {
  return React.createElement(
    React.StrictMode,
    null,
    React.createElement(Provider, {
      store: targetStore,
      children: React.createElement(ClockProvider, {
        now: () => 1_000,
        children: React.createElement(App, {
          attachmentResources: targetResources,
          canChoosePaths: true,
          digest: async () => `sha256:${"ab".repeat(32)}`,
          followConversationChoices: (show) => {
            show({
              catalog: {
                agents: [],
                environments: [],
                commandHosts: [],
                nativeHooks: { claude: "off", codex: "off" },
              },
              chosenAgent: undefined,
            })
            return () => undefined
          },
        }),
      }),
    }),
  )
}

function replacementProvider() {
  const replacementResources = createAttachmentResources()
  const effects = scenarioEffects("echo")
  const replacementSend = vi.fn(effects.submit)
  const replacementStore = makeStore({
    attachments: replacementResources,
    canChoosePaths: true,
    conversation: { ...effects, submit: replacementSend },
  })
  replacementStore.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  expect(
    replacementStore.getState().conversation.activeId,
    "a different provider begins with the same local tab identity",
  ).toBe("c0")
  store = replacementStore
  resources = replacementResources
  send = replacementSend
  return {
    store: replacementStore,
    resources: replacementResources,
    send: replacementSend,
  }
}

const active = () =>
  store
    .getState()
    .conversation.conversations.find(
      (conversation) => conversation.id === store.getState().conversation.activeId,
    )!

async function pressEnter() {
  const editor = container.querySelector<HTMLElement>('[contenteditable="true"]')
  expect(editor, "the real Markdown editor must be mounted").not.toBeNull()
  await act(async () => {
    editor!.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        code: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    )
  })
  await act(async () => {})
}

async function dropImage(url: string) {
  expect(hostBoundary.drop, "actual App host-drop subscription must exist").toBeTypeOf(
    "function",
  )
  const batch = `image-only-text-gesture-${url}`
  await act(async () => hostBoundary.batch?.(batch, "dropped"))
  await act(async () =>
    hostBoundary.drop!({
      batch,
      files: [],
      text: { plain: "", uriList: url, html: `<img src="${url}">` },
      refused: null,
    }),
  )
}

async function dropPathless(payload: { plain: string; uriList: string; html: string }) {
  expect(hostBoundary.drop, "actual App host-drop subscription must exist").toBeTypeOf(
    "function",
  )
  await act(async () =>
    hostBoundary.drop!({
      batch: "pathless-without-a-beginning",
      files: [],
      text: payload,
      refused: null,
    }),
  )
}

async function dropFolder() {
  const { target, event } = folderDropEvent()
  await act(async () => target.dispatchEvent(event))
}

function dropFolderSynchronously() {
  const { target, event } = folderDropEvent()
  flushSync(() => target.dispatchEvent(event))
}

function folderDropEvent() {
  const rootElement = container.querySelector<HTMLElement>("[data-nessa-root]")
  expect(rootElement, "the actual App drop boundary must be mounted").not.toBeNull()
  const folder = { isDirectory: true } as FileSystemEntry
  const event = new Event("drop", { bubbles: true, cancelable: true })
  Object.defineProperty(event, "dataTransfer", {
    value: {
      types: ["Files"],
      files: [],
      items: [{ kind: "file", webkitGetAsEntry: () => folder }],
    },
  })
  return { target: rootElement!, event }
}

async function enterComposerText(text: string) {
  const editor = container.querySelector<HTMLElement>('[contenteditable="true"]')
  expect(editor, "the actual Markdown editor must be mounted").not.toBeNull()
  editor!.textContent = text
  await act(async () =>
    editor!.dispatchEvent(
      new InputEvent("input", { bubbles: true, inputType: "insertText", data: text }),
    ),
  )
}

function pressEnterSynchronously() {
  const editor = container.querySelector<HTMLElement>('[contenteditable="true"]')
  expect(editor, "the real Markdown editor must be mounted").not.toBeNull()
  flushSync(() =>
    editor!.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        code: "Enter",
        bubbles: true,
        cancelable: true,
      }),
    ),
  )
}

async function pickImage(text = "include this image") {
  const nativeRead = deferred<ArrayBuffer>()
  cleanups.push(() => nativeRead.resolve(new ArrayBuffer(4)))
  hostBoundary.choose.mockResolvedValue([
    {
      path: "/Users/ada/picked.png",
      name: "picked.png",
      size: 4,
      mimeType: "image/png",
      ticket: "native-read-ticket",
    },
  ])
  hostBoundary.read.mockReturnValue(nativeRead.promise)
  await mount(text)
  await act(async () =>
    container.querySelector<HTMLButtonElement>('[aria-label="Add attachment"]')!.click(),
  )
  const addFiles = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (button) => button.textContent === "Add files",
  )
  expect(addFiles, "the actual composer tray must offer the native picker").toBeDefined()
  await act(async () => addFiles!.click())
  expect(hostBoundary.choose).toHaveBeenCalledOnce()
  expect(hostBoundary.read).toHaveBeenCalledExactlyOnceWith("native-read-ticket")
  return nativeRead
}

beforeEach(() => {
  prototypeProperties = []
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  }))
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  vi.stubGlobal("scrollBy", vi.fn())
  supplyProperty(Range.prototype, "getClientRects", () => [])
  supplyProperty(Range.prototype, "getBoundingClientRect", () => ({
    x: 0,
    y: 0,
    width: 0,
    height: 0,
    top: 0,
    right: 0,
    bottom: 0,
    left: 0,
    toJSON() {
      return {}
    },
  }))
  vi.stubGlobal(
    "URL",
    class extends URL {
      static createObjectURL() {
        return "blob:probe"
      }
      static revokeObjectURL() {}
    },
  )
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0,
    y: 0,
    width: 640,
    height: 480,
    top: 0,
    right: 640,
    bottom: 480,
    left: 0,
    toJSON() {
      return {}
    },
  })
  supplyProperty(
    Element.prototype,
    "animate",
    vi.fn(() => ({ cancel() {} })),
  )
  hostBoundary.choose.mockReset()
  hostBoundary.batch = undefined
  hostBoundary.read.mockReset()
  hostBoundary.drop = undefined
  folderBoundary.read.mockReset()
  cleanups = []
  resources = createAttachmentResources()
  const effects = scenarioEffects("echo")
  send = vi.fn(effects.submit)
  store = makeStore({
    attachments: resources,
    canChoosePaths: true,
    conversation: { ...effects, submit: send },
  })
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(async () => {
    for (const cleanup of cleanups) cleanup()
  })
  await act(async () => root.unmount())
  container.remove()
  for (const [target, name, descriptor] of prototypeProperties) {
    if (descriptor) Object.defineProperty(target, name, descriptor)
    else Reflect.deleteProperty(target, name)
  }
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

it("the actual panel's Enter key sends an ordinary text draft", async () => {
  await mount("ordinary message")
  await pressEnter()
  expect(send).toHaveBeenCalledOnce()
  expect(send.mock.calls[0]![0]).toMatchObject({
    text: "ordinary message",
    attachments: [],
    files: [],
  })
  expect(active().draft).toEqual([])
})

it("explicitly refuses a second image URL drop while the first download is pending", async () => {
  const fetching = vi.fn(
    (_url: string, input: { signal: AbortSignal }) =>
      new Promise<never>((_resolve, reject) => {
        input.signal.addEventListener("abort", () => reject(input.signal.reason), {
          once: true,
        })
      }),
  )
  vi.stubGlobal("fetch", fetching)
  // Abort at unmount settles the held fetch through the real download helper.
  await mount()
  await dropImage("https://example.test/first.png")
  expect(fetching).toHaveBeenCalledOnce()
  expect(container.textContent).toContain("Image")
  await dropImage("https://example.test/second.png")
  expect(fetching, "the concurrent image is refused, not started").toHaveBeenCalledOnce()
  expect(
    container.textContent,
    "the refused second gesture must receive its own explanation",
  ).toContain("Still reading files")
})

it("Enter waits for a picked image's native byte read and preserves the text draft", async () => {
  await pickImage()
  expect(active().draft).toEqual([{ type: "text", text: "include this image" }])
  await pressEnter()
  expect(
    send,
    "no message may leave while its picked image's byte read is pending",
  ).not.toHaveBeenCalled()
  expect(active().draft).toEqual([{ type: "text", text: "include this image" }])
  expect(container.textContent).toContain("Attachments still loading")
})

it("attaches and sends the picked image once its read and upload have settled", async () => {
  const reading = await pickImage()
  expect(
    container.querySelector('[aria-label="Getting picked.png ready"]'),
  ).not.toBeNull()
  await act(async () => reading.resolve(new ArrayBuffer(4)))
  expect(container.querySelector('[aria-label="Getting picked.png ready"]')).toBeNull()
  expect(active().draft).toContainEqual(
    expect.objectContaining({
      type: "file",
      resource: expect.objectContaining({ name: "picked.png" }),
      upload: expect.objectContaining({ status: "stored" }),
    }),
  )
  await pressEnter()
  expect(send).toHaveBeenCalledOnce()
  expect(send.mock.calls[0]![0]).toMatchObject({
    text: "include this image",
    files: [],
    attachments: [{ mimeType: "image/png", size: 4 }],
  })
})

it("applies a host drop to A after the App switches to conversation B", async () => {
  await mount()
  const destinationStore = store
  const conversationId = active().id
  const batch = "b-origin-store-drop"
  expect(hostBoundary.batch, "the App must subscribe to attachment batches").toBeTypeOf(
    "function",
  )
  await act(async () => hostBoundary.batch!(batch, "dropped"))

  await act(async () => store.dispatch(openConversation()))
  expect(active().id).not.toBe(conversationId)
  await act(async () =>
    hostBoundary.drop?.({
      batch,
      files: [
        {
          path: "/Users/ada/original.pdf",
          name: "original.pdf",
          size: 4,
          mimeType: "application/pdf",
          ticket: "original-ticket",
        },
      ],
      text: { plain: "", uriList: "", html: "" },
      refused: null,
    }),
  )

  expect(
    destinationStore
      .getState()
      .conversation.conversations.find(
        (conversation) => conversation.id === conversationId,
      )?.draft,
  ).toContainEqual(
    expect.objectContaining({
      type: "file",
      resource: expect.objectContaining({ path: "/Users/ada/original.pdf" }),
    }),
  )
})

it("pastes pathless host text into the actual focused composer", async () => {
  await mount()
  await dropPathless({ plain: "dropped prose", uriList: "", html: "" })

  expect(active().draft).toContainEqual(
    expect.objectContaining({ type: "pasted-text", text: "dropped prose" }),
  )
})

it("captures pathless host image ownership before the fetch waits", async () => {
  const response = deferred<Response>()
  vi.stubGlobal(
    "fetch",
    vi.fn(() => response.promise),
  )
  await mount()
  const originalStore = store
  const originalId = active().id
  await dropPathless({
    plain: "",
    uriList: "https://example.test/pathless.png",
    html: '<img src="https://example.test/pathless.png">',
  })
  expect(fetch).toHaveBeenCalledOnce()

  await act(async () => store.dispatch(openConversation()))
  const replacementId = active().id
  await act(async () => {
    response.resolve(
      new Response("image", {
        headers: { "content-type": "image/png" },
      }),
    )
    await new Promise((resolve) => setTimeout(resolve, 0))
  })

  expect(
    originalStore
      .getState()
      .conversation.conversations.find((conversation) => conversation.id === originalId)
      ?.draft,
  ).toContainEqual(
    expect.objectContaining({
      type: "file",
      resource: expect.objectContaining({ name: "pathless.png", mimeType: "image/png" }),
    }),
  )
  expect(
    originalStore
      .getState()
      .conversation.conversations.find(
        (conversation) => conversation.id === replacementId,
      )?.draft,
  ).toHaveLength(0)
})

it("keeps all 64 active drop owners, refuses the 65th, and consumes each result once", async () => {
  await mount()
  const originalId = active().id
  await act(async () => {
    for (let index = 0; index < 64; index += 1)
      hostBoundary.batch!(`held-${index}`, "dropped")
  })

  await act(async () => store.dispatch(openConversation()))
  const replacementId = active().id
  await act(async () => hostBoundary.batch!("overflow", "dropped"))

  const deliverFile = async (batch: string, name: string) =>
    act(async () =>
      hostBoundary.drop!({
        batch,
        files: [
          {
            path: `/Users/ada/${name}`,
            name,
            size: 4,
            mimeType: "application/pdf",
            ticket: `ticket-${name}`,
          },
        ],
        text: { plain: "", uriList: "", html: "" },
        refused: null,
      }),
    )

  // The oldest accepted result remains bound to A while B is current, and its
  // terminal result releases one of the 64 slots.
  await deliverFile("held-0", "original.pdf")
  const originalDraft = store
    .getState()
    .conversation.conversations.find(
      (conversation) => conversation.id === originalId,
    )?.draft
  expect(originalDraft).toContainEqual(
    expect.objectContaining({
      type: "file",
      resource: expect.objectContaining({ path: "/Users/ada/original.pdf" }),
    }),
  )
  expect(container.textContent).toContain("Too many selections")

  // A duplicate terminal result, the rejected 65th result, and an unknown
  // result have all lost their destination and cannot attach anywhere.
  await deliverFile("held-0", "duplicate.pdf")
  await deliverFile("overflow", "overflow.pdf")
  await deliverFile("unknown", "unknown.pdf")
  expect(
    store
      .getState()
      .conversation.conversations.find((conversation) => conversation.id === originalId)
      ?.draft,
  ).toHaveLength(1)
  expect(active().draft).toEqual([])

  // Capacity released by held-0 admits another batch on B.
  await act(async () => hostBoundary.batch!("after-release", "dropped"))
  await deliverFile("after-release", "replacement.pdf")
  expect(active().id).toBe(replacementId)
  expect(active().draft).toContainEqual(
    expect.objectContaining({
      type: "file",
      resource: expect.objectContaining({ path: "/Users/ada/replacement.pdf" }),
    }),
  )
})

it("clears a failed native image read so the retained text can be sent", async () => {
  const reading = await pickImage()
  await act(async () =>
    reading.reject({ reason: "file-unreadable", shown: "picked.png" }),
  )
  expect(container.querySelector('[aria-label="Getting picked.png ready"]')).toBeNull()
  expect(container.textContent).toContain("picked.png")
  expect(active().draft).toEqual([{ type: "text", text: "include this image" }])
  await pressEnter()
  expect(send).toHaveBeenCalledOnce()
  expect(send.mock.calls[0]![0]).toMatchObject({
    text: "include this image",
    attachments: [],
  })
})

it("keeps a native read with its original tab while another tab can send", async () => {
  await pickImage()
  const original = active().id
  await act(async () => {
    store.dispatch(openConversation())
    store.dispatch(setDraft({ draft: [{ type: "text", text: "other tab" }] }))
  })
  expect(container.querySelector('[aria-label="Getting picked.png ready"]')).toBeNull()
  await pressEnter()
  expect(send).toHaveBeenCalledOnce()
  expect(send.mock.calls[0]![0]).toMatchObject({ text: "other tab", attachments: [] })
  await act(async () => store.dispatch(setActive(original)))
  expect(
    container.querySelector('[aria-label="Getting picked.png ready"]'),
  ).not.toBeNull()
  await pressEnter()
  expect(send).toHaveBeenCalledOnce()
  expect(active().draft).toEqual([{ type: "text", text: "include this image" }])
  expect(container.textContent).toContain("Attachments still loading")
})

it("removes an existing draft file without releasing a selected image's pending read", async () => {
  await act(async () =>
    store.dispatch(
      attachFiles({
        conversationId: active().id,
        files: resources.addChosen([
          {
            path: "/Users/ada/old.pdf",
            name: "old.pdf",
            mimeType: "application/pdf",
            size: 4,
            ticket: "unused-non-image-ticket",
          },
        ]),
      }),
    ),
  )
  await pickImage()
  await act(async () =>
    container.querySelector<HTMLButtonElement>('[aria-label="Remove old.pdf"]')!.click(),
  )
  expect(active().draft.filter((part) => part.type === "file")).toEqual([])
  expect(
    container.querySelector('[aria-label="Getting picked.png ready"]'),
  ).not.toBeNull()
  await pressEnter()
  expect(send).not.toHaveBeenCalled()
  expect(container.textContent).toContain("Attachments still loading")
})

it("does not attach a read result to the replacement tab after its original closes", async () => {
  const reading = await pickImage()
  const original = active().id
  await act(async () => store.dispatch(closeConversation(original)))
  expect(active().id).not.toBe(original)
  await act(async () => reading.resolve(new ArrayBuffer(4)))
  expect(active().draft).toEqual([])
  expect(container.querySelector('[aria-label="Getting picked.png ready"]')).toBeNull()
  expect(container.textContent).toContain("That conversation was closed")
  expect(
    container.querySelector<HTMLButtonElement>('[aria-label="Add attachment"]')!.disabled,
  ).toBe(false)
})

it("ignores a native read settling after the panel unmounts", async () => {
  const reading = await pickImage()
  const add = vi.spyOn(resources, "add")
  await act(async () => root.unmount())
  await act(async () => reading.resolve(new ArrayBuffer(4)))
  expect(add).not.toHaveBeenCalled()
  expect(active().draft).toEqual([{ type: "text", text: "include this image" }])
  expect(send).not.toHaveBeenCalled()
})

it("does not let A's pending walk block B's same-ID draft after a provider swap", async () => {
  const readA = deferred<File[]>()
  folderBoundary.read.mockReturnValue(readA.promise)
  await mount()
  await enterComposerText("send from B")
  await dropFolder()
  expect(folderBoundary.read).toHaveBeenCalledOnce()

  const replacement = replacementProvider()
  await act(async () => {
    renderAppSynchronously(replacement.store, replacement.resources)
    pressEnterSynchronously()
  })

  expect(
    send,
    "a walk in A's retired resource scope must not keep B's composer pending",
  ).toHaveBeenCalledOnce()
  expect(send.mock.calls[0]![0]).toMatchObject({ text: "send from B" })
  expect(active().id).toBe("c0")
  expect(replacement.send.mock.calls[0]![0]).toMatchObject({ text: "send from B" })
  await act(async () => readA.resolve([new File(["a"], "A.txt")]))
})

it("starts a second same-ID folder walk in the replacement provider immediately", async () => {
  const readA = deferred<File[]>()
  const readB = deferred<File[]>()
  folderBoundary.read.mockImplementation(() =>
    folderBoundary.read.mock.calls.length === 1 ? readA.promise : readB.promise,
  )
  const originalStore = store
  const originalId = active().id
  await mount()
  await dropFolder()

  const replacement = replacementProvider()
  await act(async () => {
    renderAppSynchronously(replacement.store, replacement.resources)
    dropFolderSynchronously()
  })

  expect(
    folderBoundary.read,
    "the new provider's folder gesture must not be refused as A's duplicate",
  ).toHaveBeenCalledTimes(2)

  await act(async () => readB.resolve([new File(["b"], "B.txt")]))
  expect(
    replacement.store
      .getState()
      .conversation.conversations.find((conversation) => conversation.id === originalId)
      ?.draft,
  ).toContainEqual(
    expect.objectContaining({ resource: expect.objectContaining({ name: "B.txt" }) }),
  )
  await act(async () => readA.resolve([new File(["a"], "A.txt")]))
  expect(
    originalStore
      .getState()
      .conversation.conversations.find((conversation) => conversation.id === originalId)
      ?.draft,
  ).toContainEqual(
    expect.objectContaining({ resource: expect.objectContaining({ name: "A.txt" }) }),
  )
})
