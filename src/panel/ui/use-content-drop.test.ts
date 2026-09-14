import type * as React from "react"
import { afterEach, expect, it, vi } from "vitest"
import { createContentDropHandlers } from "./use-content-drop"

afterEach(() => vi.unstubAllGlobals())

function setup(types: string[], payload: Record<string, string> = {}, fileCount = 0) {
  const actions = {
    addImageUrl: vi.fn(async () => {}),
    focusComposer: vi.fn(),
    pasteAttachment: vi.fn(),
  }
  const setDragging = vi.fn()
  const event = {
    dataTransfer: {
      types,
      files: { length: fileCount },
      getData: (type: string) => payload[type] ?? "",
      dropEffect: "none",
    },
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
    currentTarget: { contains: () => false },
    relatedTarget: null,
  }
  return {
    actions,
    setDragging,
    event,
    dragEvent: event as unknown as React.DragEvent<HTMLDivElement>,
    handlers: createContentDropHandlers(actions, setDragging),
  }
}

it("accepts native text at dragenter and inserts the dropped payload as pasted text", () => {
  const test = setup(["text/plain"], { "text/plain": "  selected text\n" })
  test.handlers.onDragEnterCapture(test.dragEvent)
  expect(test.event.preventDefault).toHaveBeenCalledOnce()
  expect(test.setDragging).toHaveBeenLastCalledWith(true)
  test.handlers.onDragOverCapture(test.dragEvent)
  expect(test.event.dataTransfer.dropEffect).toBe("copy")
  test.handlers.onDropCapture(test.dragEvent)
  expect(test.actions.pasteAttachment).toHaveBeenCalledWith("  selected text\n")
  expect(test.actions.focusComposer).toHaveBeenCalledOnce()
  expect(test.event.stopPropagation).toHaveBeenCalledOnce()
  expect(test.setDragging).toHaveBeenLastCalledWith(false)
})

it("handles native URI payloads even when Files is advertised without bytes", () => {
  const test = setup(["Files", "text/uri-list"], {
    "text/uri-list": "https://example.com/article",
  })
  test.handlers.onDropCapture(test.dragEvent)
  expect(test.actions.pasteAttachment).toHaveBeenCalledWith("https://example.com/article")
  expect(test.event.preventDefault).toHaveBeenCalledOnce()
})

it("leaves actual files for FileDropZone rather than also attaching their text URL", () => {
  const test = setup(
    ["Files", "text/uri-list"],
    {
      "text/uri-list": "https://example.com/photo.png",
    },
    1,
  )
  test.handlers.onDropCapture(test.dragEvent)
  expect(test.event.stopPropagation).not.toHaveBeenCalled()
  expect(test.actions.addImageUrl).not.toHaveBeenCalled()
  expect(test.actions.pasteAttachment).not.toHaveBeenCalled()
  expect(test.setDragging).toHaveBeenLastCalledWith(false)
})

it("routes a remote image to attachment loading and prevents URL navigation", () => {
  const test = setup(["text/uri-list"], {
    "text/uri-list": "https://example.com/photo.png",
  })
  test.handlers.onDropCapture(test.dragEvent)
  expect(test.actions.addImageUrl).toHaveBeenCalledWith("https://example.com/photo.png")
  expect(test.actions.pasteAttachment).not.toHaveBeenCalled()
  expect(test.event.preventDefault).toHaveBeenCalledOnce()
  expect(test.event.stopPropagation).toHaveBeenCalledOnce()
})

it("clears feedback when the drag leaves the panel", () => {
  const test = setup(["text/plain"])
  test.handlers.onDragLeaveCapture(test.dragEvent)
  expect(test.setDragging).toHaveBeenCalledWith(false)
})

it("pastes selected prose rather than the advertised inline-image URI", () => {
  vi.stubGlobal(
    "DOMParser",
    class {
      parseFromString() {
        return {
          body: { textContent: "Selected prose with an emoji" },
          querySelector: () => ({ getAttribute: () => "https://example.com/emoji.png" }),
        }
      }
    },
  )
  const test = setup(["text/html", "text/plain", "text/uri-list"], {
    "text/html":
      '<p>Selected prose with an emoji<img src="https://example.com/emoji.png"></p>',
    "text/plain": "  Selected prose with an emoji\n",
    "text/uri-list": "https://example.com/emoji.png",
  })
  test.handlers.onDropCapture(test.dragEvent)
  expect(test.actions.pasteAttachment).toHaveBeenCalledWith(
    "  Selected prose with an emoji\n",
  )
  expect(test.actions.addImageUrl).not.toHaveBeenCalled()
  expect(test.event.preventDefault).toHaveBeenCalledOnce()
  expect(test.event.stopPropagation).toHaveBeenCalledOnce()
})
