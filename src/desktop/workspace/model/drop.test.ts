import { describe, expect, it } from "vitest"
import {
  aimAt,
  dropOutcome,
  edgeHug,
  edgeReach,
  paneAt,
  headingWindow,
  pointerVelocity,
  restAfter,
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
    // Its middle is what the sides leave: a third of the way down is still the top.
    expect(zoneAt(at(160, 450), tall)).toBe("center")
    expect(zoneAt(at(160, 250), tall)).toBe("top")
    expect(zoneAt(at(160, 320), tall)).toBe("center")
  })

  it("reaches a third of the way in from each side, held to 90–300px and never past the middle", () => {
    expect(edgeReach({ width: 600, height: 600 }, "left")).toBeCloseTo(200)
    expect(edgeReach({ width: 2000, height: 1400 }, "left")).toBe(300)
    expect(edgeReach({ width: 2000, height: 1400 }, "top")).toBe(300)
    expect(edgeReach({ width: 200, height: 200 }, "top")).toBe(90)
    expect(edgeReach({ width: 150, height: 150 }, "right")).toBe(75)
    // 400px in from the left of a large pane is its middle; 250px is its side.
    expect(zoneAt(at(400, 700), { width: 2000, height: 1400 })).toBe("center")
    expect(zoneAt(at(250, 700), { width: 2000, height: 1400 })).toBe("left")
  })

  it("reads a drag down a tall pane as below by two-thirds of the way, and the same sideways and up", () => {
    // The person's pane: 845 by 1200, a drag straight down its middle.
    const pane = { width: 845, height: 1200 }
    const down = { x: 0, y: 0.6 }
    const firstBelow = (
      from: number,
      to: number,
      step: number,
      point: (at: number) => [number, number],
      velocity: { x: number; y: number },
      extent: number,
    ) => {
      let zone: Zone | null = null
      for (let place = from; step > 0 ? place <= to : place >= to; place += step) {
        const [x, y] = point(place)
        zone = zoneAt(at(x, y, velocity), pane, zone)
        if (zone !== "center")
          return { zone, share: (step > 0 ? place : extent - place) / extent }
      }
      return null
    }
    const below = firstBelow(600, 1200, 4, (y) => [422, y], down, 1200)
    expect(below?.zone).toBe("bottom")
    expect(below?.share).toBeLessThanOrEqual(0.67)
    // Up, the same distance from the top.
    const above = firstBelow(600, 0, -4, (y) => [422, y], { x: 0, y: -0.6 }, 1200)
    expect(above?.zone).toBe("top")
    expect(above?.share).toBeLessThanOrEqual(0.67)
    // At rest, a third of the way from the foot.
    expect(zoneAt(at(422, 850), pane)).toBe("center")
    expect(zoneAt(at(422, 905), pane)).toBe("bottom")
    // Sideways, the side it heads for, well before the edge.
    const right = firstBelow(422, 845, 4, (x) => [x, 600], { x: 0.6, y: 0 }, 845)
    expect(right?.zone).toBe("right")
    expect(right?.share).toBeLessThanOrEqual(0.67)
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

  it("moves a sideways drag near a tall narrow pane's top beside it, never above, at any depth", () => {
    // The reviewer's pane: 352 by 884, a drag moving steadily left across it.
    const narrow = { width: 352, height: 884 }
    const leftward = { x: -0.6, y: 0.02 }
    for (const y of [30, 45, 60, 90]) {
      let zone: Zone | null = null
      const seen = new Set<Zone>()
      for (let x = 350; x >= 2; x -= 4) {
        zone = zoneAt(at(x, y, leftward), narrow, zone)
        seen.add(zone)
      }
      expect([...seen], `${y}px down`).not.toContain("top")
      expect([...seen], `${y}px down`).not.toContain("bottom")
      expect(zone).toBe("left")
    }
    // Hugging the top edge, the top is still in reach.
    expect(zoneAt(at(176, edgeHug - 4, leftward), narrow)).toBe("top")
    // At rest there, it is the top, as the diagonals say.
    expect(zoneAt(at(176, 50), narrow)).toBe("top")
  })

  it("moves an up-or-down drag near a wide short pane's side above or below it, never beside", () => {
    const low = { width: 884, height: 352 }
    const downward = { x: 0.02, y: 0.6 }
    for (const x of [30, 45, 60, 90]) {
      let zone: Zone | null = null
      const seen = new Set<Zone>()
      for (let y = 2; y <= 350; y += 4) {
        zone = zoneAt(at(x, y, downward), low, zone)
        seen.add(zone)
      }
      expect([...seen], `${x}px in`).not.toContain("left")
      expect([...seen], `${x}px in`).not.toContain("right")
      expect(zone).toBe("bottom")
    }
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

  it("holds the zone the pointer is in when it jitters sideways across the boundary", () => {
    // In the bottom zone of a pane, shaken left and right: a plain sideways
    // heading keeps the pointer out of the bottom, but not out once it is in.
    const pane = { width: 546, height: 800 }
    let zone: Zone | null = zoneAt(at(273, 640), pane)
    expect(zone).toBe("bottom")
    for (let shake = 0; shake < 20; shake++)
      zone = zoneAt(
        at(273 + (shake % 2 ? 6 : -6), 640, { x: shake % 2 ? 0.6 : -0.6, y: 0 }),
        pane,
        zone,
      )
    expect(zone).toBe("bottom")
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
    const reach = edgeReach(square, "top")
    expect(zoneAt(at(300, reach - 4), square, "center")).toBe("center")
    expect(zoneAt(at(300, reach + 4), square, "top")).toBe("top")
    expect(zoneAt(at(300, reach + zoneHold + 1), square, "top")).toBe("center")
  })

  it("reads the pointer's heading from the last tenth of a second, and none once it has rested", () => {
    expect(pointerVelocity([], 0)).toEqual({ x: 0, y: 0 })
    expect(pointerVelocity([{ x: 5, y: 5, t: 10 }], 10)).toEqual({ x: 0, y: 0 })
    const path = [
      { x: 0, y: 900, t: 0 },
      { x: 100, y: 100, t: 400 },
      { x: 140, y: 100, t: 450 },
      { x: 180, y: 100, t: 500 },
    ]
    // An old sample, long before the window, says nothing of where it heads now.
    expect(pointerVelocity(path, 500)).toEqual({ x: 0.8, y: 0 })
    expect(pointerVelocity(path, 500 + restAfter - 1)).toEqual({ x: 0.8, y: 0 })
    // Still for `restAfter`, it heads nowhere.
    expect(pointerVelocity(path, 500 + restAfter)).toEqual({ x: 0, y: 0 })
    expect(headingWindow).toBeGreaterThanOrEqual(80)
    expect(headingWindow).toBeLessThanOrEqual(120)
  })
})

describe("where a drag aims", () => {
  // Two columns: a on the left, b over c on the right, in a 2000 by 1300 grid.
  const grid: [number, { left: number; top: number; width: number; height: number }][] = [
    [1, { left: 0, top: 0, width: 996, height: 1300 }],
    [2, { left: 1004, top: 0, width: 996, height: 646 }],
    [3, { left: 1004, top: 654, width: 996, height: 646 }],
  ]
  const targets = { grid: { left: 0, top: 0, width: 2000, height: 1300 }, panes: grid }

  it("reads a gutter as the pane nearest it, held to its edge", () => {
    expect(paneAt({ x: 1001, y: 300 }, grid)).toMatchObject({ key: 2, x: 0, y: 300 })
    expect(paneAt({ x: 1500, y: 650 }, grid)?.key).toBe(2)
    expect(paneAt({ x: 10, y: 10 }, [])).toBeNull()
  })

  it("aims at the pointer: the pane and zone under it, nothing off the grid or with no pane in sight", () => {
    const still = (x: number, y: number) => [{ x, y, t: 0 }]
    // The middle of a tall pane is its middle: the copy's centre is the pointer.
    expect(aimAt(still(498, 650), 0, null, targets)).toEqual({
      target: 1,
      zone: "center",
    })
    expect(aimAt(still(1500, 1250), 0, null, targets)).toEqual({
      target: 3,
      zone: "bottom",
    })
    expect(aimAt(still(1001, 300), 0, null, targets)).toMatchObject({ target: 2 })
    expect(aimAt(still(2100, 300), 0, null, targets)).toBeNull()
    expect(aimAt(still(1500, -5), 0, null, targets)).toBeNull()
    expect(aimAt(still(498, 650), 0, null, null)).toBeNull()
    expect(aimAt([], 0, null, targets)).toBeNull()
  })

  it("holds the zone it had only on the same pane", () => {
    // 200px in from pane 1's left, at rest, is its middle — unless it was its left.
    const path = [{ x: 305, y: 650, t: 0 }]
    expect(aimAt(path, 0, null, targets)).toEqual({ target: 1, zone: "center" })
    expect(aimAt(path, 0, { target: 1, zone: "left" }, targets)).toEqual({
      target: 1,
      zone: "left",
    })
    expect(aimAt(path, 0, { target: 2, zone: "left" }, targets)).toEqual({
      target: 1,
      zone: "center",
    })
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
