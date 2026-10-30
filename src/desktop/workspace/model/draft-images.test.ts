import { describe, expect, it } from "vitest"

import { draftImagesRefusal, imagesLine } from "./draft-images"

const limits = { images: 2, bytes: 10, totalBytes: 15, types: ["image/png"] }
const png = (size: number) => ({ size, mimeType: "image/png" })

describe("images on one message", () => {
  it("takes all of what is offered or none, and says which bound refused it", () => {
    expect(draftImagesRefusal([], [png(10)], limits)).toBe(null)
    expect(draftImagesRefusal([], [{ size: 1, mimeType: "image/tiff" }], limits)).toBe(
      "unsupported",
    )
    expect(draftImagesRefusal([png(1)], [png(1), png(1)], limits)).toBe("too-many")
    expect(draftImagesRefusal([], [png(11)], limits)).toBe("too-large")
    expect(draftImagesRefusal([png(8)], [png(8)], limits)).toBe("too-heavy")
  })

  it("names a message of images alone by what it carries", () => {
    expect(imagesLine(undefined)).toBe("")
    expect(imagesLine([{}])).toBe("Image")
    expect(imagesLine([{}, {}, {}])).toBe("3 images")
  })
})
