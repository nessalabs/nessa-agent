// @vitest-environment jsdom
/**
 * What a tile says about its own upload.
 *
 * The rules about uploads are tested as plain functions elsewhere. This is what
 * somebody actually sees: a band of light on the edge of the tile while the
 * bytes are going, the picture never covered by it, one green answer when the
 * gateway has them, and an ordinary tile after that.
 *
 * What the green band's own removal waits for — the browser saying the settling
 * animation is over — cannot be reached from here: jsdom has no `AnimationEvent`,
 * so React does not register `animationend` at all and a dispatched one is heard
 * by nothing. The band going is therefore the one step of this a person has to
 * see for themselves, and it is also why a status change clears it as well (the
 * case below), so a band whose animation never ended cannot outlive the state
 * that put it there. Nessa has no test that opens the real window; see issue #89.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

// The tile reads a few pure rules from the conversation. Its barrel also
// exports the conversation's components, which need the whole design system
// resolved; `testing` is the same rules without them.
vi.mock("../../conversation", () => import("../../conversation/testing"))

import type { FileAttachment } from "../../conversation/testing"
import { AttachmentTile } from "./attachment-tile"

const file = (upload: FileAttachment["upload"]): FileAttachment => ({
  type: "file",
  id: "f",
  name: "photo.png",
  mimeType: "image/png",
  size: 3,
  previewUrl: "blob:test",
  upload,
})

let container: HTMLDivElement
let root: Root

async function show(upload: FileAttachment["upload"]) {
  await React.act(async () => {
    root.render(
      React.createElement(AttachmentTile, {
        file: file(upload),
        onOpen: vi.fn(),
        onRemove: vi.fn(),
        onRetry: vi.fn(),
      }),
    )
  })
}

const ring = () => container.querySelector(".nessa-upload-ring")
const picture = () => container.querySelector("img")

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

it("puts the band on the tile's edge while the bytes go, and leaves the picture alone", async () => {
  await show({ status: "uploading" })
  expect(ring()?.getAttribute("data-state")).toBe("uploading")
  expect(ring()?.getAttribute("role")).toBe("status")
  // The picture is the thing being looked at: it is shown, and not dimmed.
  expect(picture()?.getAttribute("src")).toBe("blob:test")
  expect(container.querySelector(".opacity-60")).toBeNull()
})

it("answers once in green when the gateway has the image", async () => {
  await show({ status: "uploading" })
  await show({
    status: "stored",
    image: { digest: "sha256:0", mimeType: "image/png", size: 1 },
  })
  expect(ring()?.getAttribute("data-state")).toBe("settled")
  // Green is an answer, not a label: unlike the band that says an upload is
  // under way, it announces nothing a screen reader has not already been told.
  expect(ring()?.getAttribute("role")).toBeNull()
})

it("takes the green back as soon as the file is doing something else", async () => {
  await show({ status: "uploading" })
  await show({
    status: "stored",
    image: { digest: "sha256:0", mimeType: "image/png", size: 1 },
  })
  expect(ring()?.getAttribute("data-state")).toBe("settled")
  // Its own animation ending is what usually takes it away. This is the other
  // way, and the one that matters if that never arrives: an upload started
  // again is not a finished one, so the band from the last time cannot stay.
  await show({ status: "uploading" })
  expect(ring()?.getAttribute("data-state")).toBe("uploading")
  await show({ status: "failed", reason: "unavailable" })
  expect(ring()).toBeNull()
})

it("says nothing on a tile that was already stored when it arrived", async () => {
  // A tab returned to, or a draft restored: the file is ready, and an ordinary
  // tile is how that reads. Green here would announce something that already
  // happened, every time the tile came back.
  await show({
    status: "stored",
    image: { digest: "sha256:0", mimeType: "image/png", size: 1 },
  })
  expect(ring()).toBeNull()
  await show({ status: "not-started" })
  expect(ring()).toBeNull()
  // Waiting its turn is dimmed, and still has no band.
  expect(container.querySelector(".opacity-60")).not.toBeNull()
})
