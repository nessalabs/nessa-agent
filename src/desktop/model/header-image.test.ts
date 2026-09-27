import { describe, expect, it } from "vitest"
import {
  checkHeaderImage,
  clampHeaderFraming,
  defaultHeaderFraming,
  headerImageMaxBytes,
  panHeaderFraming,
  parseHeaderFraming,
  placeHeaderImage,
} from "./header-image"

describe("checkHeaderImage", () => {
  it.each(["image/png", "image/jpeg", "image/gif", "image/webp"])(
    "accepts %s",
    (type) => {
      expect(checkHeaderImage({ type, size: 1024 })).toEqual({ ok: true })
    },
  )

  it("refuses a file that is not an image", () => {
    expect(checkHeaderImage({ type: "video/mp4", size: 1024 })).toEqual({
      ok: false,
      reason: "not-an-image",
    })
    expect(checkHeaderImage({ type: "", size: 1024 })).toEqual({
      ok: false,
      reason: "not-an-image",
    })
  })

  it("refuses an image over the size limit, and accepts one exactly at it", () => {
    expect(
      checkHeaderImage({ type: "image/gif", size: headerImageMaxBytes + 1 }),
    ).toEqual({
      ok: false,
      reason: "too-large",
    })
    expect(checkHeaderImage({ type: "image/gif", size: headerImageMaxBytes })).toEqual({
      ok: true,
    })
  })
})

const frame = { width: 1000, height: 250 }
// Covering a 1000 × 250 header takes this picture at half size: 1000 × 500.
const wide = { width: 2000, height: 1000 }

describe("placeHeaderImage", () => {
  it("covers the header and centres the picture by default", () => {
    expect(placeHeaderImage(defaultHeaderFraming, frame, wide)).toEqual({
      width: 1000,
      height: 500,
      left: 0,
      top: -125,
    })
  })

  it("enlarges by the zoom around the focal point", () => {
    const placed = placeHeaderImage({ x: 0.5, y: 0.5, zoom: 2 }, frame, wide)
    expect(placed).toEqual({ width: 2000, height: 1000, left: -500, top: -375 })
  })
})

describe("clampHeaderFraming", () => {
  it("never lets an edge of the header show past the picture", () => {
    const clamped = clampHeaderFraming({ x: 0, y: 1, zoom: 1 }, frame, wide)
    const placed = placeHeaderImage(clamped, frame, wide)
    expect(placed.left).toBeLessThanOrEqual(0)
    expect(placed.top).toBeLessThanOrEqual(0)
    expect(placed.left + placed.width).toBeGreaterThanOrEqual(frame.width)
    expect(placed.top + placed.height).toBeGreaterThanOrEqual(frame.height)
  })

  it("keeps the zoom between 1 and 3", () => {
    expect(clampHeaderFraming({ x: 0.5, y: 0.5, zoom: 9 }, frame, wide).zoom).toBe(3)
    expect(clampHeaderFraming({ x: 0.5, y: 0.5, zoom: 0.2 }, frame, wide).zoom).toBe(1)
  })
})

describe("panHeaderFraming", () => {
  it("moves the picture with the pointer", () => {
    const moved = panHeaderFraming({ x: 0.5, y: 0.5, zoom: 1 }, 0, 50, frame, wide)
    expect(placeHeaderImage(moved, frame, wide).top).toBeCloseTo(-75)
  })

  it("stops at the picture's edge", () => {
    const moved = panHeaderFraming({ x: 0.5, y: 0.5, zoom: 1 }, 0, 9999, frame, wide)
    expect(placeHeaderImage(moved, frame, wide).top).toBeCloseTo(0)
  })
})

describe("parseHeaderFraming", () => {
  it("reads a stored framing", () => {
    expect(parseHeaderFraming({ x: 0.2, y: 0.7, zoom: 1.5 })).toEqual({
      x: 0.2,
      y: 0.7,
      zoom: 1.5,
    })
  })

  it.each([null, "x", { x: "1", y: 0, zoom: 1 }, { x: Number.NaN, y: 0, zoom: 1 }])(
    "falls back to the default for %j",
    (value) => {
      expect(parseHeaderFraming(value)).toEqual(defaultHeaderFraming)
    },
  )
})
