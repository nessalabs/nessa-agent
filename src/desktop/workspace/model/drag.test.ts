/**
 * A drag's life, row by row of ADR 238 › Drag and drop's table: one test at
 * least for each row, in its order.
 */
import { describe, expect, it } from "vitest"
import {
  idle,
  keyToDrag,
  liftDistance,
  stepDrag,
  type DragEvent,
  type DragPhase,
} from "./drag"
import type { Aim, Carried, PointerSample, Targets } from "./drop"
import { restAfter } from "./drop"

const pane: Carried = { kind: "pane", pane: 1 }
const session: Carried = { kind: "session", sessionId: "s" }

/** Two panes side by side in an 1100 × 800 grid; pane 2 is 546 × 800 from x 554. */
const targets: Targets = {
  grid: { left: 0, top: 0, width: 1100, height: 800 },
  panes: [
    [1, { left: 0, top: 0, width: 546, height: 800 }],
    [2, { left: 554, top: 0, width: 546, height: 800 }],
  ],
}

const at = (x: number, y: number, t: number): PointerSample => ({ x, y, t })
const press = (carried: Carried = pane): DragEvent => ({
  kind: "press",
  carried,
  pointerId: 1,
  at: at(60, 16, 0),
})
const move = (
  x: number,
  y: number,
  t: number,
  over: Targets | null = targets,
): DragEvent => ({
  kind: "move",
  pointerId: 1,
  at: at(x, y, t),
  targets: over,
})
const offersAll = () => true
const release = (
  x: number,
  y: number,
  t: number,
  offers: (aim: Aim) => boolean = offersAll,
  over: Targets | null = targets,
): DragEvent => ({
  kind: "release",
  pointerId: 1,
  at: at(x, y, t),
  targets: over,
  offers,
})

const run = (events: readonly DragEvent[], from: DragPhase = idle) =>
  events.reduce(stepDrag, from)

/** Pressed, then carried to the middle of pane 2 and left there. */
const carrying = (over: Targets | null = targets) =>
  run([
    press(),
    move(90, 40, 10, over),
    move(827, 400, 20, over),
    { kind: "still", t: 20 + restAfter, targets: over },
  ])

describe("a press", () => {
  it("idle: a press becomes pressed; nothing else leaves idle", () => {
    expect(run([press()])).toMatchObject({ kind: "pressed", carried: pane, pointerId: 1 })
    for (const event of [
      move(90, 40, 10),
      release(90, 40, 10),
      { kind: "escape" },
      { kind: "lost" },
      { kind: "changed" },
      { kind: "landed" },
    ] as const)
      expect(stepDrag(idle, event)).toBe(idle)
  })

  it("pressed: a move under the lift distance stays a press", () => {
    const pressed = run([press()])
    expect(stepDrag(pressed, move(60 + liftDistance - 1, 16, 5))).toBe(pressed)
  })

  it("pressed: a move of the lift distance or more carries it, aiming at nothing yet", () => {
    expect(run([press(), move(60 + liftDistance, 16, 5)])).toMatchObject({
      kind: "carrying",
      aim: null,
      path: [{ x: 64, y: 16, t: 5 }],
    })
  })

  it("pressed: a release is a click, back to idle", () => {
    expect(run([press(), release(61, 16, 5)])).toBe(idle)
  })

  it("pressed: Escape, another key, a lost pointer or a change let the press go — a later move starts nothing", () => {
    for (const end of [
      { kind: "escape" },
      keyToDrag("w"),
      { kind: "lost" },
      { kind: "changed" },
    ] as const) {
      if (!end) throw new Error("a key that ends nothing")
      const after = run([press(), end])
      expect(after).toBe(idle)
      expect(run([move(400, 400, 20)], after)).toBe(idle)
    }
  })

  it("follows only the pointer that pressed", () => {
    const pressed = run([press()])
    expect(stepDrag(pressed, { ...move(400, 400, 5), pointerId: 2 } as DragEvent)).toBe(
      pressed,
    )
  })
})

describe("carrying", () => {
  it("a move decides the zone from the pointer itself", () => {
    // The middle of pane 2: a swap's zone.
    expect(carrying()).toMatchObject({
      kind: "carrying",
      aim: { target: 2, zone: "center" },
    })
    // Near its right edge: its right.
    expect(run([move(1090, 400, 500)], carrying())).toMatchObject({
      aim: { target: 2, zone: "right" },
    })
  })

  it("a move off the grid aims at nothing; in a gutter, at the nearest pane", () => {
    expect(run([move(827, 900, 500)], carrying())).toMatchObject({ aim: null })
    expect(run([move(552, 400, 500)], carrying())).toMatchObject({ aim: { target: 2 } })
  })

  it("with no pane in sight (the overview over them), nothing is ever a target", () => {
    const blind = carrying(null)
    expect(blind).toMatchObject({ kind: "carrying", aim: null })
    expect(run([{ kind: "still", t: 1000, targets: null }], blind)).toBe(blind)
    expect(run([release(827, 400, 1000, offersAll, null)], blind)).toEqual({
      kind: "cancelling",
      how: "home",
    })
  })

  it("still for restAfter: the zone is decided again as at rest", () => {
    // Heading right fast into pane 2, 220px from its right edge: the heading
    // reaches the right side (1.4 × 182px)…
    const fast = run([
      press(),
      move(90, 40, 0),
      move(800, 400, 10),
      move(840, 400, 20),
      move(880, 400, 30),
    ])
    expect(fast).toMatchObject({ aim: { target: 2, zone: "right" } })
    // …at rest there it is the middle (182px reach, 12px held: 194px).
    const settled = stepDrag(fast, { kind: "still", t: 30 + restAfter, targets })
    expect(settled).toMatchObject({ aim: { target: 2, zone: "center" } })
    // Nudged a pixel after resting, it stays where it settled.
    expect(stepDrag(settled, move(881, 400, 30 + restAfter + 150))).toMatchObject({
      aim: { target: 2, zone: "center" },
    })
    // A move to where it already is restarts nothing.
    expect(stepDrag(fast, move(880, 400, 100))).toBe(fast)
    // Before the heading ages out, a still changes nothing.
    expect(stepDrag(fast, { kind: "still", t: 30 + restAfter - 1, targets })).toBe(fast)
  })

  it("release on a zone that offers something drops it there", () => {
    expect(run([release(827, 400, 500)], carrying())).toEqual({
      kind: "dropping",
      carried: pane,
      aim: { target: 2, zone: "center" },
    })
  })

  it("release after a pause drops on the zone at rest, not the one the heading reached", () => {
    const fast = run([
      press(),
      move(90, 40, 0),
      move(800, 400, 10),
      move(840, 400, 20),
      move(880, 400, 30),
    ])
    expect(run([release(880, 400, 30 + restAfter)], fast)).toMatchObject({
      kind: "dropping",
      aim: { zone: "center" },
    })
    // Let go while still moving: the zone it was heading for.
    expect(run([release(880, 400, 40)], fast)).toMatchObject({
      kind: "dropping",
      aim: { zone: "right" },
    })
  })

  it("release where nothing is offered — no zone, or one the room refuses — goes home", () => {
    const home = { kind: "cancelling", how: "home" }
    expect(run([release(827, 900, 500)], carrying())).toEqual(home)
    expect(run([release(827, 400, 500, () => false)], carrying())).toEqual(home)
  })

  it("Escape, or a lost pointer, goes home — and the press goes with it", () => {
    for (const end of [{ kind: "escape" }, { kind: "lost" }] as const) {
      const after = run([end], carrying())
      expect(after).toEqual({ kind: "cancelling", how: "home" })
      // The reviewer's order: lost, then a move and a release over a zone.
      const later = run(
        [move(827, 400, 600), release(827, 400, 610), { kind: "landed" }],
        after,
      )
      expect(later).toBe(idle)
      expect(run([move(827, 400, 700), release(827, 400, 710)], later)).toBe(idle)
    }
  })

  it("a change — a command key, a resize, the store, Settings — ends it at once", () => {
    expect(run([{ kind: "changed" }], carrying())).toEqual({
      kind: "cancelling",
      how: "at-once",
    })
    expect(keyToDrag("w")).toEqual({ kind: "changed" })
    expect(keyToDrag("0")).toEqual({ kind: "changed" })
    expect(keyToDrag("ArrowLeft")).toEqual({ kind: "changed" })
    expect(keyToDrag("Escape")).toEqual({ kind: "escape" })
    // A modifier on its own commands nothing.
    for (const key of ["Meta", "Shift", "Alt", "Control"])
      expect(keyToDrag(key)).toBeNull()
  })

  it("carries a session the same way", () => {
    expect(
      run([press(session), move(90, 40, 10), move(827, 400, 20), release(827, 400, 30)]),
    ).toMatchObject({ kind: "dropping", carried: session, aim: { target: 2 } })
  })
})

describe("dropping and cancelling", () => {
  it("the drop's own change, a press, or any other event changes nothing until it lands", () => {
    const dropping = run([release(827, 400, 500)], carrying())
    const home = run([{ kind: "escape" }], carrying())
    for (const phase of [dropping, home])
      for (const event of [
        { kind: "changed" },
        press(),
        move(100, 100, 600),
        release(100, 100, 600),
        { kind: "escape" },
        { kind: "lost" },
        { kind: "still", t: 900, targets },
      ] as const)
        expect(stepDrag(phase, event)).toBe(phase)
  })

  it("its flight landing ends it", () => {
    expect(run([release(827, 400, 500), { kind: "landed" }], carrying())).toBe(idle)
    expect(run([{ kind: "changed" }, { kind: "landed" }], carrying())).toBe(idle)
  })
})
