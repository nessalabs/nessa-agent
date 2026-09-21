// @vitest-environment jsdom
/**
 * What the panel does with a drop the host describes.
 *
 * The host half of this is tested in `src-tauri/src/attachments/dropping.rs`
 * and the seam between them by `refusal.rs`'s two-way name check. What is left
 * — and what is here — is the routing: a drag of files must reach the *same*
 * call a picker selection reaches, and a drag of anything else must reach the
 * readers that already knew what to do with it.
 *
 * Nothing here can perform a drag. What it can do is hand the hook exactly what
 * the host emits, which is a serialized struct with a test of its own on the
 * other side.
 */
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"

const { onAttachmentDragging, onAttachmentDropped } = vi.hoisted(() => ({
  onAttachmentDragging: vi.fn(),
  onAttachmentDropped: vi.fn(),
}))
vi.mock("../../host", () => ({ onAttachmentDragging, onAttachmentDropped }))

import { useHostDrop } from "./use-host-drop"

type Dropped = Parameters<Parameters<typeof onAttachmentDropped>[0]>[0]

/** Where the panel says it is, which `Surface` re-reads on every render. */
let here = "c1"
/** What `beganBatch` recorded, which is the whole of what it is for. */
const bound = new Map<string, string>()
const actions = {
  addChosenFiles: vi.fn(),
  beganBatch: vi.fn((batch: string, conversationId: string) => {
    bound.set(batch, conversationId)
  }),
  conversationOf: vi.fn((batch: string) => bound.get(batch) ?? here),
  addImageUrl: vi.fn(),
  focusComposer: vi.fn(),
  pasteAttachment: vi.fn(),
  refuse: vi.fn(),
}
/** The handler the hook registered, once it has subscribed. */
let deliver: (dropped: Dropped) => void
let drag: (over: { dragging: boolean; batch: string | null }) => void
let container: HTMLElement
let root: Root
let dragging: boolean

function Surface() {
  dragging = useHostDrop(actions, here).dragging
  return null
}

const file = (name: string, mimeType: string) => ({
  path: `/Users/ada/${name}`,
  name,
  size: 4,
  mimeType,
  ticket: `t-${name}`,
})

const dropOf = (dropped: Partial<Dropped>): Dropped => ({
  batch: "b1",
  files: [],
  text: { plain: "", uriList: "", html: "" },
  refused: null,
  ...dropped,
})

beforeEach(async () => {
  here = "c1"
  bound.clear()
  for (const action of Object.values(actions)) action.mockClear()
  onAttachmentDragging.mockReset()
  onAttachmentDropped.mockReset()
  onAttachmentDragging.mockImplementation(
    (handler: (over: { dragging: boolean; batch: string | null }) => void) => {
      drag = handler
      return Promise.resolve(() => {})
    },
  )
  onAttachmentDropped.mockImplementation((handler: (dropped: Dropped) => void) => {
    deliver = handler
    return Promise.resolve(() => {})
  })
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
  await act(async () => {
    root.render(React.createElement(Surface))
  })
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

it("sends dropped files down the same route a picked selection takes", async () => {
  // Both, in one drop, because the type is what decides and the gesture is
  // not: the hook hands the whole selection over untouched and
  // `addChosenFiles` splits it, exactly as it does for `+`.
  const dropped = [file("shot.png", "image/png"), file("ContentView.swift", "")]

  await act(async () => deliver(dropOf({ files: dropped })))

  expect(actions.addChosenFiles).toHaveBeenCalledWith(dropped, "c1")
  // Nothing was classified here. A second opinion about what an image is is
  // the defect this feature already had once.
  expect(actions.pasteAttachment).not.toHaveBeenCalled()
  expect(actions.addImageUrl).not.toHaveBeenCalled()
  expect(actions.refuse).not.toHaveBeenCalled()
})

it("pastes dragged prose, which no longer reaches the page any other way", async () => {
  // The page receives no `drop` event at all now, so this is the only route
  // dragged text has. The host read these three off the drag pasteboard.
  await act(async () =>
    deliver(
      dropOf({
        text: { plain: "some prose", uriList: "", html: "<p>some prose</p>" },
      }),
    ),
  )

  expect(actions.pasteAttachment).toHaveBeenCalledWith("some prose")
  expect(actions.focusComposer).toHaveBeenCalled()
  expect(actions.addChosenFiles).not.toHaveBeenCalled()
})

it("downloads an image dragged off a web page, which also has no other route", async () => {
  await act(async () =>
    deliver(
      dropOf({
        text: {
          plain: "",
          uriList: "https://example.com/cat.png",
          html: '<img src="https://example.com/cat.png">',
        },
      }),
    ),
  )

  expect(actions.addImageUrl).toHaveBeenCalledWith("https://example.com/cat.png")
  expect(actions.pasteAttachment).not.toHaveBeenCalled()
})

it("says why a drop was turned away, in the panel's own words", async () => {
  // The host's reason, through the one function that knows its vocabulary.
  await act(async () =>
    deliver(dropOf({ refused: { reason: "folder-empty", shown: null, detail: null } })),
  )

  expect(actions.refuse).toHaveBeenCalledWith({ reason: "empty-folder" })
  expect(actions.addChosenFiles).not.toHaveBeenCalled()
})

it("draws the drop target while a drag is over the panel, and stops on the drop", async () => {
  expect(dragging).toBe(false)

  await act(async () => drag({ dragging: true, batch: null }))
  expect(dragging).toBe(true)

  // The drop itself puts it down, rather than waiting for a `leave` that a
  // completed drag never sends.
  await act(async () => deliver(dropOf({ files: [file("a.pdf", "application/pdf")] })))
  expect(dragging).toBe(false)
})

it("answers the handlers that are current when the drop lands", async () => {
  // The subscription is made once, at mount. A drop three tabs later must not
  // reach the handlers that were current then — which is what capturing the
  // actions in the effect would do.
  const later = vi.fn()
  await act(async () => {
    actions.addChosenFiles = later
    root.render(React.createElement(Surface))
  })

  await act(async () => deliver(dropOf({ files: [file("a.pdf", "application/pdf")] })))

  expect(later).toHaveBeenCalled()
})

it("attaches to the conversation the drop landed on, not the one open when it finishes", async () => {
  // The gesture's moment: a drag ends over `c1`, and the host names the drop.
  await act(async () => drag({ dragging: false, batch: "b7" }))
  expect(actions.beganBatch).toHaveBeenCalledWith("b7", "c1")

  // Three quarters of a minute of fetching later, somebody is reading another
  // conversation. The files finally arrive.
  await act(async () => {
    here = "c2"
    root.render(React.createElement(Surface))
  })
  await act(async () =>
    deliver(dropOf({ batch: "b7", files: [file("a.pdf", "application/pdf")] })),
  )

  // They join the draft they were dropped on. Reading the open tab here put
  // them — and the tile that blocks its send — over a draft they were never
  // going to join, while the one they belonged to showed nothing.
  expect(actions.addChosenFiles).toHaveBeenCalledWith(
    [file("a.pdf", "application/pdf")],
    "c1",
  )
})

it("binds each drop to its own conversation, so two in flight do not swap drafts", async () => {
  await act(async () => drag({ dragging: false, batch: "b1" }))
  await act(async () => {
    here = "c2"
    root.render(React.createElement(Surface))
  })
  await act(async () => drag({ dragging: false, batch: "b2" }))

  // Out of order, which is what two fetches of different sizes do.
  await act(async () => deliver(dropOf({ batch: "b2", files: [file("b.pdf", "")] })))
  await act(async () => deliver(dropOf({ batch: "b1", files: [file("a.pdf", "")] })))

  expect(actions.addChosenFiles.mock.calls.map(([, target]) => target)).toEqual([
    "c2",
    "c1",
  ])
})

it("does not bind a drag that is only passing over the panel", async () => {
  // Only a drop is named. An entering drag has no batch, and binding the open
  // tab to a drop that never happened would leave a dead entry behind.
  await act(async () => drag({ dragging: true, batch: null }))

  expect(actions.beganBatch).not.toHaveBeenCalled()
})
