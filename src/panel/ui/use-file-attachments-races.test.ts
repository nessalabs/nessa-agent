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
import { Provider } from "react-redux"
import { beforeEach, afterEach, expect, it, vi } from "vitest"
import type { DroppedOnPanel } from "../../host"

const hostBoundary = vi.hoisted(() => ({
  choose: vi.fn(),
  read: vi.fn(),
  drop: undefined as undefined | ((payload: DroppedOnPanel) => void),
}))

vi.mock("../../host", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../host")>()),
  hasNativeHost: () => true,
  chooseAttachmentFiles: hostBoundary.choose,
  readAttachmentBytes: hostBoundary.read,
  onAttachmentReadying: () => Promise.resolve(() => {}),
  onAttachmentBatch: () => Promise.resolve(() => {}),
  onAttachmentDragging: () => Promise.resolve(() => {}),
  onAttachmentDropped: (handler: (payload: DroppedOnPanel) => void) => {
    hostBoundary.drop = handler
    return Promise.resolve(() => {})
  },
}))

import { App } from "./app"
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

async function mount(text = "") {
  if (text) store.dispatch(setDraft({ draft: [{ type: "text", text }] }))
  await act(async () =>
    root.render(
      React.createElement(
        React.StrictMode,
        null,
        React.createElement(Provider, {
          store,
          children: React.createElement(App, {
            attachmentResources: resources,
            canChoosePaths: true,
            digest: async () => `sha256:${"ab".repeat(32)}`,
            loadConversationChoices: async () => ({
              catalog: { agents: [] },
              chosenAgent: undefined,
            }),
          }),
        }),
      ),
    ),
  )
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
  await act(async () =>
    hostBoundary.drop!({
      batch: "image-only-text-gesture",
      files: [],
      text: { plain: "", uriList: url, html: `<img src="${url}">` },
      refused: null,
    }),
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
  hostBoundary.read.mockReset()
  hostBoundary.drop = undefined
  cleanups = []
  resources = createAttachmentResources()
  const effects = scenarioEffects("echo")
  send = vi.fn(effects.send)
  store = makeStore({
    attachments: resources,
    canChoosePaths: true,
    conversation: { ...effects, send },
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
      name: "picked.png",
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
