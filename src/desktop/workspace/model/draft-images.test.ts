import { describe, expect, it } from "vitest"

import { draftImagesRefusal, draftKind, imagesLine } from "./draft-images"

const limits = {
  images: 2,
  bytes: 10,
  totalBytes: 15,
  types: ["image/png"],
  files: 2,
  fileBytes: 20,
}
const png = (size: number) => ({
  size,
  kind: "image" as const,
  mimeType: "image/png",
  name: "a.png",
})
const file = (size: number) => ({
  size,
  kind: "file" as const,
  mimeType: "text/plain",
  name: "notes.txt",
})

describe("attachments on one message", () => {
  it("takes all of what is offered or none, and says which bound refused it", () => {
    expect(draftImagesRefusal([], [png(10)], limits)).toBe(null)
    expect(draftImagesRefusal([png(1)], [png(1), png(1)], limits)).toBe("too-many")
    expect(draftImagesRefusal([], [png(11)], limits)).toBe("too-large")
    expect(draftImagesRefusal([png(8)], [png(8)], limits)).toBe("too-heavy")
  })

  it("holds files to their own bounds, beside the images", () => {
    // A file larger than any image may be is still a file a message carries.
    expect(draftImagesRefusal([png(10), png(5)], [file(18)], limits)).toBe(null)
    expect(draftImagesRefusal([file(1)], [file(1), file(1)], limits)).toBe(
      "too-many-files",
    )
    expect(draftImagesRefusal([file(15)], [file(6)], limits)).toBe("too-heavy-files")
    expect(draftImagesRefusal([], [file(0)], limits)).toBe("empty")
    // The gateway stores any image as an image: one no message names is
    // refused here rather than sent as a file it would convert or refuse.
    expect(draftImagesRefusal([], [{ ...file(1), mimeType: "image/heic" }], limits)).toBe(
      "unsupported",
    )
  })

  it("sends one of the image encodings as an image and anything else as a file", () => {
    expect(draftKind("image/png", limits)).toBe("image")
    expect(draftKind("image/heic", limits)).toBe("file")
    expect(draftKind("application/pdf", limits)).toBe("file")
  })

  it("names a message of attachments alone by what it carries", () => {
    expect(imagesLine(undefined)).toBe("")
    expect(imagesLine([png(1)])).toBe("Image")
    expect(imagesLine([png(1), png(1), png(1)])).toBe("3 images")
    expect(imagesLine([file(1)])).toBe("notes.txt")
    expect(imagesLine([png(1), file(1)])).toBe("2 attachments")
  })
})
