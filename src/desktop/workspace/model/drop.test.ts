import { describe, expect, it } from "vitest"
import {
  centreInset,
  dropOutcome,
  headingWindow,
  pointerVelocity,
  zoneAt,
  zoneHold,
} from "./drop"
import { singlePane, splitPane, type Zone } from "./pane-layout"

const room = { width: 1100, height: 800, spare: 0 }
const zones: readonly Zone[] = ["left", "right", "top", "bottom", "center"]

const still = { x: 0, y: 0 }
const at = (x: number, y: number, velocity = still) => ({ x, y, velocity })
const tall = { width: 320, height: 900 }
const wide = { width: 1200, height: 300 }
const square = { width: 600, height: 600 }

describe("where on a pane a drop lands", () => {
  it("lands on the side it is nearest in pixels, and in the middle well within every edge", () => {
    expect(zoneAt(at(20, 300), square)).toBe("left")
    expect(zoneAt(at(580, 300), square)).toBe("right")
    expect(zoneAt(at(300, 20), square)).toBe("top")
    expect(zoneAt(at(300, 580), square)).toBe("bottom")
    expect(zoneAt(at(300, 300), square)).toBe("center")
  })

  it("keeps a tall narrow pane's top and bottom short: a quarter of the way down is a side, not the top", () => {
    // 225px from the top, 60px from the left: the old shares called this "top".
    expect(zoneAt(at(60, 225), tall)).toBe("left")
    expect(zoneAt(at(270, 225), tall)).toBe("right")
    expect(zoneAt(at(160, 40), tall)).toBe("top")
    expect(zoneAt(at(160, 870), tall)).toBe("bottom")
    // Its middle is a size in pixels, most of its height.
    expect(zoneAt(at(160, 450), tall)).toBe("center")
    expect(zoneAt(at(160, 150), tall)).toBe("center")
  })

  it("sizes the middle in pixels: a large pane's edges stay a hand's width, a small one's never vanish", () => {
    expect(centreInset({ width: 2000, height: 1400 })).toBe(120)
    expect(centreInset({ width: 150, height: 150 })).toBe(48)
    // 130px in from the left of a large pane is already its middle.
    expect(zoneAt(at(130, 700), { width: 2000, height: 1400 })).toBe("center")
  })

  it("keeps a wide short pane's sides short in the same way", () => {
    expect(zoneAt(at(250, 60), wide)).toBe("top")
    expect(zoneAt(at(250, 250), wide)).toBe("bottom")
    expect(zoneAt(at(30, 150), wide)).toBe("left")
    expect(zoneAt(at(600, 150), wide)).toBe("center")
  })

  it("takes a sideways sweep near the top to the side it heads for", () => {
    // 50px down, 60px from the right: at rest that is the top (50 < 60)…
    expect(zoneAt(at(540, 50), square)).toBe("top")
    // …moving right at 0.8px/ms, it is the right side it is reaching for.
    expect(zoneAt(at(540, 50, { x: 0.8, y: 0 }), square)).toBe("right")
    // Moving up, the top it is.
    expect(zoneAt(at(540, 50, { x: 0, y: -0.8 }), square)).toBe("top")
  })

  it("never takes a side out of reach for the one the pointer is at, however it heads", () => {
    // 82px in from a narrow pane's left, sweeping right: still the left.
    expect(zoneAt(at(82, 450, { x: 0.8, y: 0 }), tall)).toBe("left")
  })

  it("splits a diagonal by the diagonal, and a diagonal heading by what it heads for", () => {
    expect(zoneAt(at(30, 40), square)).toBe("left")
    expect(zoneAt(at(40, 30), square)).toBe("top")
    // On the diagonal itself, heading up and a little left: the top.
    expect(zoneAt(at(35, 35, { x: -0.2, y: -0.8 }), square)).toBe("top")
    expect(zoneAt(at(35, 35, { x: -0.8, y: -0.2 }), square)).toBe("left")
  })

  it("does not let a resting pointer's jitter flip the zone at a boundary", () => {
    // Either side of the top/right diagonal by a pixel or two, the zone it is in holds.
    let zone = zoneAt(at(560, 38), square)
    expect(zone).toBe("top")
    for (const [x, y] of [
      [562, 40],
      [561, 37],
      [563, 41],
      [560, 38],
    ])
      zone = zoneAt(at(x, y), square, zone)
    expect(zone).toBe("top")
    // Well past it, the other takes over.
    expect(zoneAt(at(585, 60), square, zone)).toBe("right")
    // The centre's edge holds too.
    const inset = centreInset(square)
    expect(zoneAt(at(300, inset - 4), square, "center")).toBe("center")
    expect(zoneAt(at(300, inset + 4), square, "top")).toBe("top")
    expect(zoneAt(at(300, inset + zoneHold + 1), square, "top")).toBe("center")
  })

  it("reads the pointer's heading from the last tenth of a second", () => {
    expect(pointerVelocity([])).toEqual({ x: 0, y: 0 })
    expect(pointerVelocity([{ x: 5, y: 5, t: 10 }])).toEqual({ x: 0, y: 0 })
    // An old sample, long before the window, says nothing of where it heads now.
    expect(
      pointerVelocity([
        { x: 0, y: 900, t: 0 },
        { x: 100, y: 100, t: 400 },
        { x: 140, y: 100, t: 450 },
        { x: 180, y: 100, t: 500 },
      ]),
    ).toEqual({ x: 0.8, y: 0 })
    expect(headingWindow).toBeGreaterThanOrEqual(80)
    expect(headingWindow).toBeLessThanOrEqual(120)
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
