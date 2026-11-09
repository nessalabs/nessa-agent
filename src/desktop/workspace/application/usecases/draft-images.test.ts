import { describe, expect, it } from "vitest"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import type { DraftImage } from "../../model/draft-images"
import type { TurnMessage } from "../../model/transcript"
import { releasedImageUrls } from "./draft-images"

const image = (url: string): DraftImage => ({
  id: url,
  name: `${url}.png`,
  mimeType: "image/png",
  size: 1,
  url,
})

const sent = (images: DraftImage[]): TurnMessage => ({
  id: "m",
  role: "user",
  at: 1,
  parts: [],
  localImages: images,
})

describe("a pasted image's bytes", () => {
  it("are let go once nothing names them: not a composer, and not a message waiting to be held", () => {
    const pasted: WorkspaceState = {
      ...initialWorkspace,
      composerImages: { s: [image("a"), image("b")] },
    }
    // Sent: both now on the message, so nothing is let go.
    const sending: WorkspaceState = {
      ...pasted,
      composerImages: {},
      outbox: { s: [sent([image("a"), image("b")])] },
    }
    expect(releasedImageUrls(pasted, sending)).toEqual([])
    // The conversation holds the message: its images are the conversation's now.
    expect(releasedImageUrls(sending, { ...sending, outbox: {} })).toEqual(["a", "b"])
    // One taken off the composer goes; the other stays.
    expect(
      releasedImageUrls(pasted, { ...pasted, composerImages: { s: [image("b")] } }),
    ).toEqual(["a"])
  })

  it("are not looked at when neither the composers nor the outbox changed", () => {
    const pasted: WorkspaceState = {
      ...initialWorkspace,
      composerImages: { s: [image("a")] },
    }
    expect(
      releasedImageUrls(pasted, { ...pasted, composerText: { s: "typed" } }),
    ).toEqual([])
  })
})
