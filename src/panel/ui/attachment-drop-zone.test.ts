// @vitest-environment jsdom
/**
 * A file dropped on the panel, through the real drop zone.
 *
 * `droppedFilesRefusal` is a plain function and tested as one, but a pure
 * function nothing calls explains nothing: the bug in the report needed the
 * zone's own bounds, the zone's own rejection reasons, and this panel's
 * handler, and it is the join of those three that was wrong. So this drops
 * actual files on an actual `FileDropZone` and reads what comes out the other
 * side. Deleting the handler, or the bounds, fails here.
 *
 * jsdom has no `DataTransfer` and no `DragEvent`, so the drop carries a stand-in
 * with the three things the zone reads: `types`, `files`, and `items`. An empty
 * `items` is what a browser gives when the entry API is unavailable, which the
 * zone documents as "take the payload at face value" — exactly the path a
 * plain file drop takes.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("../../conversation", () => import("../../conversation/testing"))

import { MAX_ATTACHMENT_BYTES, MAX_DRAFT_ATTACHMENTS } from "../../conversation/testing"
import { AttachmentDropZone } from "./attachment-drop-zone"

const onFiles = vi.fn()
const onRefused = vi.fn()

/** A File of a given weight without allocating it: only `size` and `name` are read. */
const weighing = (name: string, size: number) =>
  ({ name, size, type: "video/mp4" }) as File

let container: HTMLDivElement
let root: Root

async function drop(files: readonly File[]) {
  const event = new Event("drop", { bubbles: true, cancelable: true })
  Object.defineProperty(event, "dataTransfer", {
    value: { types: ["Files"], files, items: [] },
  })
  await React.act(async () => {
    container.querySelector("[data-panel]")!.dispatchEvent(event)
  })
  // The zone resolves the payload before it delivers; let that settle.
  await React.act(async () => {})
}

beforeEach(async () => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.clearAllMocks()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
  await React.act(async () => {
    root.render(
      React.createElement(AttachmentDropZone, {
        onFiles,
        onRefused,
        children: React.createElement("div", { "data-panel": true }, "panel"),
      }),
    )
  })
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("refuses an oversized file by its weight, and never sends it to the picker", async () => {
  // The 720 MB .mp4 from the report. What used to happen here was one sentence
  // for every rejection — "Try selecting them with +" — and the + picker holds
  // a file to this same bound, so it would have been refused again.
  await drop([weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)])
  expect(onRefused).toHaveBeenCalledExactlyOnceWith({
    reason: "file-too-large",
    names: ["holiday.mp4"],
  })
  expect(onFiles).not.toHaveBeenCalled()
})

it("attaches what passed and refuses the rest of the same drop", async () => {
  const small = weighing("small.mp4", 8)
  await drop([small, weighing("holiday.mp4", MAX_ATTACHMENT_BYTES + 1)])
  expect(onFiles).toHaveBeenCalledExactlyOnceWith([small])
  expect(onRefused).toHaveBeenCalledExactlyOnceWith({
    reason: "file-too-large",
    names: ["holiday.mp4"],
  })
})

it("refuses more files than a draft holds as a count, not a weight", async () => {
  const many = Array.from({ length: MAX_DRAFT_ATTACHMENTS + 3 }, (_, index) =>
    weighing(`${index}.mp4`, 8),
  )
  await drop(many)
  expect(onFiles).toHaveBeenCalledExactlyOnceWith(many.slice(0, MAX_DRAFT_ATTACHMENTS))
  expect(onRefused).toHaveBeenCalledExactlyOnceWith({ reason: "too-many-files" })
})

it("says nothing about a drop it took whole", async () => {
  const files = [weighing("a.mp4", 8), weighing("b.mp4", 8)]
  await drop(files)
  expect(onFiles).toHaveBeenCalledExactlyOnceWith(files)
  expect(onRefused).not.toHaveBeenCalled()
})
