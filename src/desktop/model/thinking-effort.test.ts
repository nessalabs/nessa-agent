import { describe, expect, it } from "vitest"
import {
  levelAfterKey,
  placePopover,
  popoverSpacing,
  fractionAlong,
  nearestLevel,
  positionAt,
} from "./thinking-effort"

describe("levelAfterKey", () => {
  it("steps one level with the arrows: right and up are more thinking", () => {
    expect(levelAfterKey("ArrowRight", 1, 4)).toBe(2)
    expect(levelAfterKey("ArrowUp", 1, 4)).toBe(2)
    expect(levelAfterKey("ArrowLeft", 1, 4)).toBe(0)
    expect(levelAfterKey("ArrowDown", 1, 4)).toBe(0)
    expect(levelAfterKey("PageUp", 1, 4)).toBe(2)
    expect(levelAfterKey("PageDown", 1, 4)).toBe(0)
  })

  it("goes to either end with Home and End", () => {
    expect(levelAfterKey("Home", 2, 4)).toBe(0)
    expect(levelAfterKey("End", 1, 4)).toBe(3)
  })

  it("leaves a key alone when it would not move, or is not one of its keys", () => {
    expect(levelAfterKey("ArrowRight", 3, 4)).toBeUndefined()
    expect(levelAfterKey("ArrowLeft", 0, 4)).toBeUndefined()
    expect(levelAfterKey("Home", 0, 4)).toBeUndefined()
    expect(levelAfterKey("End", 3, 4)).toBeUndefined()
    expect(levelAfterKey("Enter", 1, 4)).toBeUndefined()
    expect(levelAfterKey("a", 1, 4)).toBeUndefined()
  })

  it("moves nowhere when there are no levels", () => {
    expect(levelAfterKey("ArrowRight", 0, 0)).toBeUndefined()
    expect(levelAfterKey("End", 0, 0)).toBeUndefined()
  })
})

describe("positionAt", () => {
  // Four levels on a track from 100 to 400: one every 100px.
  it("is where the pointer stands in levels, between them while dragged", () => {
    expect(positionAt(100, 100, 300, 4)).toBe(0)
    expect(positionAt(250, 100, 300, 4)).toBe(1.5)
    expect(positionAt(400, 100, 300, 4)).toBe(3)
  })

  it("is the end the pointer runs past", () => {
    expect(positionAt(20, 100, 300, 4)).toBe(0)
    expect(positionAt(900, 100, 300, 4)).toBe(3)
  })

  it("is the only level there is, or the first on a track not laid out", () => {
    expect(positionAt(250, 100, 300, 1)).toBe(0)
    expect(positionAt(250, 100, 0, 4)).toBe(0)
  })
})

describe("nearestLevel", () => {
  it("snaps a position to the level nearest it", () => {
    expect(nearestLevel(1.49)).toBe(1)
    expect(nearestLevel(1.5)).toBe(2)
    expect(nearestLevel(2.2)).toBe(2)
  })
})

describe("fractionAlong", () => {
  it("runs from 0 at the least to 1 at the most", () => {
    expect([0, 1, 2, 3, 4].map((level) => fractionAlong(level, 5))).toEqual([
      0, 0.25, 0.5, 0.75, 1,
    ])
    expect(fractionAlong(1.5, 4)).toBe(0.5)
  })

  it("stands a model's only level at the end", () => {
    expect(fractionAlong(0, 1)).toBe(1)
  })
})

describe("placePopover", () => {
  const popover = { width: 248, height: 120 }
  const window = { width: 1280, height: 800 }
  const { gap, margin } = popoverSpacing

  it("sits above the chip with its trailing edge on the chip's", () => {
    const chip = { left: 1000, top: 700, width: 40, height: 30 }
    expect(placePopover(chip, popover, window)).toEqual({
      x: 1040 - 248,
      y: 700 - gap - 120,
      side: "above",
    })
  })

  it("shifts along to stay inside the window", () => {
    const nearLeft = { left: 20, top: 700, width: 40, height: 30 }
    expect(placePopover(nearLeft, popover, window).x).toBe(margin)
    const nearRight = { left: 1270, top: 700, width: 40, height: 30 }
    expect(placePopover(nearRight, popover, window).x).toBe(1280 - margin - 248)
  })

  it("goes below only when above would leave the window and below would not", () => {
    const high = { left: 1000, top: 60, width: 40, height: 30 }
    expect(placePopover(high, popover, window)).toMatchObject({
      y: 60 + 30 + gap,
      side: "below",
    })
    // Above, it would sit inside the window yet closer to its edge than the margin.
    const nearTop = { left: 1000, top: margin - 1 + gap + 120, width: 40, height: 30 }
    expect(placePopover(nearTop, popover, window).side).toBe("below")
    const clear = { left: 1000, top: margin + gap + 120, width: 40, height: 30 }
    expect(placePopover(clear, popover, window).side).toBe("above")
    // Room for neither: it stays above, the side a composer's controls expect.
    const cramped = { left: 1000, top: 60, width: 40, height: 30 }
    expect(placePopover(cramped, popover, { width: 1280, height: 200 }).side).toBe(
      "above",
    )
  })
})
