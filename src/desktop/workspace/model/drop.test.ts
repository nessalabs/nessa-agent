import { describe, expect, it } from "vitest"
import { dropOutcome, zoneAt } from "./drop"
import { singlePane, splitPane, type Zone } from "./pane-layout"

const room = { width: 1100, height: 800, spare: 0 }
const zones: readonly Zone[] = ["left", "right", "top", "bottom", "center"]

describe("where on a pane a drop lands", () => {
  it("lands on the side it is near, the middle otherwise", () => {
    expect(zoneAt(0.1, 0.5)).toBe("left")
    expect(zoneAt(0.9, 0.5)).toBe("right")
    expect(zoneAt(0.5, 0.05)).toBe("top")
    expect(zoneAt(0.5, 0.95)).toBe("bottom")
    expect(zoneAt(0.5, 0.5)).toBe("center")
  })
})

describe("what a drop leaves", () => {
  const two = splitPane(singlePane("a"), 1, "right", "b")

  it("swaps two panes in the middle, and moves one to a side", () => {
    expect(dropOutcome(two, { kind: "pane", pane: 2 }, 1, "center", room)).toMatchObject({
      does: "swap",
      lands: 2,
    })
    const moved = dropOutcome(two, { kind: "pane", pane: 2 }, 1, "top", room)
    expect(moved?.does).toBe("move")
    expect(moved?.layout.columns).toHaveLength(1)
  })

  it("puts a session in a new pane on a side, minted the layout's next key, or in the pane's place", () => {
    const split = dropOutcome(two, { kind: "session", sessionId: "c" }, 1, "bottom", room)
    expect(split).toMatchObject({ does: "split", lands: two.nextKey })
    expect(
      dropOutcome(two, { kind: "session", sessionId: "c" }, 1, "center", room),
    ).toMatchObject({
      does: "replace",
      lands: 1,
    })
  })

  it("goes to a session already on screen, whatever the zone", () => {
    for (const zone of zones)
      expect(
        dropOutcome(two, { kind: "session", sessionId: "b" }, 1, zone, room),
      ).toMatchObject({
        does: "go-to",
        lands: 2,
      })
  })

  it("offers nothing where the room refuses, or for a pane dropped on itself", () => {
    const narrow = { width: 500, height: 300, spare: 0 }
    expect(
      dropOutcome(two, { kind: "session", sessionId: "c" }, 1, "left", narrow),
    ).toBeNull()
    expect(dropOutcome(two, { kind: "pane", pane: 1 }, 1, "left", room)).toBeNull()
  })
})
