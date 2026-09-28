import { describe, expect, it } from "vitest"
import { titleBreathingRoom, titlePlacement, titlePlacementHold } from "./column-title"

describe("a column's title placement", () => {
  it("sits inline in the titlebar row when it fits with room to breathe", () => {
    expect(titlePlacement({ room: 300, title: 110, now: null })).toBe("inline")
    expect(
      titlePlacement({ room: 110 + titleBreathingRoom, title: 110, now: null }),
    ).toBe("inline")
  })

  it("goes to its own row below when it does not", () => {
    expect(titlePlacement({ room: 77, title: 110, now: null })).toBe("below")
    // Fits bare, but with no room to breathe: below.
    expect(titlePlacement({ room: 115, title: 110, now: "below" })).toBe("below")
  })

  it("holds inline near the threshold, so a dragged edge does not flip it every pixel", () => {
    const edge = 110 + titleBreathingRoom
    expect(titlePlacement({ room: edge - 1, title: 110, now: "below" })).toBe("below")
    expect(titlePlacement({ room: edge - 1, title: 110, now: "inline" })).toBe("inline")
    expect(
      titlePlacement({ room: edge - titlePlacementHold, title: 110, now: "inline" }),
    ).toBe("inline")
    expect(
      titlePlacement({ room: edge - titlePlacementHold - 1, title: 110, now: "inline" }),
    ).toBe("below")
  })
})
