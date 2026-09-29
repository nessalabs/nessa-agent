/**
 * A drag's life, row by row of ADR 238 › Drag and drop's table: one test at
 * least for each row, in its order.
 */
import { describe, expect, it } from "vitest"
import {
  copyShape,
  idle,
  keyToDrag,
  liftDistance,
  primaryAlone,
  stepDrag,
  type DragEvent,
  type DragPhase,
} from "./drag"
import type { Aim, Carried, PointerSample, Targets } from "./drop"
import { restAfter } from "./drop"

const pane: Carried = { kind: "pane", pane: 1 }
const item: Carried = { kind: "item", item: "s" }

/** Two panes side by side in an 1100 × 800 grid; pane 2 is 546 × 800 from x 554. */
const targets: Targets = {
  grid: { left: 0, top: 0, width: 1100, height: 800 },
  panes: [
    [1, { left: 0, top: 0, width: 546, height: 800 }],
    [2, { left: 554, top: 0, width: 546, height: 800 }],
  ],
  covered: [],
  refused: new Map(),
}

/** What the page made for the drag: here, a name. */
type Made = string
type Event = DragEvent<Made>
type Phase = DragPhase<Made>

const at = (x: number, y: number, t: number): PointerSample => ({ x, y, t })
const press = (carried: Carried = pane): Event => ({
  kind: "press",
  carried,
  pointerId: 1,
  at: at(60, 16, 0),
})
const ready: Event = { kind: "ready", made: "the copy" }
const move = (
  x: number,
  y: number,
  t: number,
  over: Targets | null = targets,
  buttons = primaryAlone,
): Event => ({
  kind: "move",
  pointerId: 1,
  buttons,
  at: at(x, y, t),
  targets: over,
})
const middleOf2: Aim = { target: 2, zone: "center", within: "center" }
/** A release of the carrying pointer at 827, 400 (pane 2's middle), with what the page shows as it lifts. */
const release = (
  shown: Aim | null = middleOf2,
  x = 827,
  y = 400,
  over: Targets | null = targets,
  t = 500,
): Event => ({
  kind: "release",
  pointerId: 1,
  at: at(x, y, t),
  targets: over,
  shown,
})

const run = (events: readonly Event[], from: Phase = idle) =>
  events.reduce<Phase>(stepDrag, from)

/** Pressed, its copy made, then carried to the middle of pane 2 and left there. */
const carrying = (over: Targets | null = targets) =>
  run([
    press(),
    ready,
    move(90, 40, 10, over),
    move(827, 400, 20, over),
    { kind: "still", t: 20 + restAfter, targets: over },
  ])

describe("a press", () => {
  it("idle: a press becomes pressed, with nothing made yet; nothing else leaves idle", () => {
    expect(run([press()])).toMatchObject({
      kind: "pressed",
      carried: pane,
      pointerId: 1,
      made: null,
    })
    for (const event of [
      ready,
      move(90, 40, 10),
      release(),
      { kind: "escape" },
      { kind: "lost" },
      { kind: "changed" },
      { kind: "landed" },
    ] as const)
      expect(stepDrag<Made>(idle, event)).toBe(idle)
  })

  it("pressed: the copy made is held in the phase, once", () => {
    const made = run([press(), ready])
    expect(made).toMatchObject({ kind: "pressed", made: "the copy" })
    expect(stepDrag(made, { kind: "ready", made: "another" })).toBe(made)
  })

  it("pressed: nothing to carry on the page (no panes laid out) lets the press go", () => {
    expect(run([press(), { kind: "changed" }])).toBe(idle)
  })

  it("pressed, nothing made yet: a move of any length stays a press", () => {
    const pressed = run([press()])
    expect(stepDrag(pressed, move(400, 400, 5))).toBe(pressed)
    // Made, the next move lifts it where the pointer is.
    expect(run([ready, move(401, 400, 6)], pressed)).toMatchObject({
      kind: "carrying",
      path: [{ x: 401, y: 400, t: 6 }],
      made: "the copy",
    })
  })

  it("pressed: another pointer's move, or its own under the lift distance, stays a press", () => {
    const pressed = run([press(), ready])
    expect(stepDrag(pressed, move(60 + liftDistance - 1, 16, 5))).toBe(pressed)
    expect(stepDrag(pressed, { ...move(400, 400, 5), pointerId: 2 } as Event)).toBe(
      pressed,
    )
  })

  it("pressed: a move of the lift distance or more carries it, aiming at nothing yet", () => {
    expect(run([press(), ready, move(60 + liftDistance, 16, 5)])).toMatchObject({
      kind: "carrying",
      aim: null,
      path: [{ x: 64, y: 16, t: 5 }],
    })
  })

  it("pressed: a release of its pointer is a click, back to idle; another's changes nothing", () => {
    expect(run([press(), release(null)])).toBe(idle)
    const pressed = run([press(), ready])
    expect(stepDrag(pressed, { ...release(null), pointerId: 2 } as Event)).toBe(pressed)
  })

  it("pressed: Escape, another key, another button, a lost pointer or a change let the press go — a later move starts nothing", () => {
    for (const end of [
      { kind: "escape" },
      keyToDrag("w"),
      move(61, 16, 5, targets, 3),
      move(61, 16, 5, targets, 0),
      { kind: "lost" },
      { kind: "changed" },
    ] as const) {
      if (!end) throw new Error("a key that ends nothing")
      const after = run([press(), ready, end])
      expect(after).toBe(idle)
      expect(run([move(400, 400, 20)], after)).toBe(idle)
    }
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

  it("a move over a side column aims at nothing, and a release there goes home", () => {
    const peeked = { ...targets, covered: [{ left: 0, top: 0, width: 256, height: 800 }] }
    const over = run([move(100, 400, 500, peeked)], carrying(peeked))
    expect(over).toMatchObject({ kind: "carrying", aim: null })
    expect(run([release(null)], over)).toEqual({
      kind: "cancelling",
      how: "home",
    })
  })

  it("another pointer's move, or a move to where it already is, changes nothing", () => {
    const carried = carrying()
    expect(stepDrag(carried, { ...move(100, 100, 600), pointerId: 2 } as Event)).toBe(
      carried,
    )
    expect(stepDrag(carried, move(827, 400, 600))).toBe(carried)
  })

  it("with no pane in sight (the overview over them), nothing is ever a target", () => {
    const blind = carrying(null)
    expect(blind).toMatchObject({ kind: "carrying", aim: null })
    expect(run([{ kind: "still", t: 1000, targets: null }], blind)).toBe(blind)
    expect(run([release(middleOf2, 827, 400, null)], blind)).toEqual({
      kind: "cancelling",
      how: "home",
    })
  })

  it("still for restAfter: the zone is decided again, and the one it is in holds", () => {
    // Heading right fast into pane 2, 220px from its right edge: the heading
    // reaches the right side (1.4 × 182px)…
    const fast = run([
      press(),
      ready,
      move(90, 40, 0),
      move(800, 400, 10),
      move(840, 400, 20),
      move(880, 400, 30),
    ])
    expect(fast).toMatchObject({ aim: { target: 2, zone: "right" } })
    // …and coming to rest there does not take it away: the side it is in
    // keeps the heading's reach, so the panes do not swing back as the hand stops.
    const settled = stepDrag(fast, { kind: "still", t: 30 + restAfter, targets })
    expect(settled).toBe(fast)
    // Nudged a pixel after resting, it stays where it settled.
    expect(stepDrag(settled, move(881, 400, 30 + restAfter + 150))).toMatchObject({
      aim: { target: 2, zone: "right" },
    })
    // Arriving there at rest, the way the pane has come still counts: moved
    // right from its place, the right side reaches as a heading does.
    const slow = run([press(), ready, move(90, 40, 0), move(880, 400, 400)])
    expect(stepDrag(slow, { kind: "still", t: 400 + restAfter, targets })).toMatchObject({
      aim: { target: 2, zone: "right" },
    })
    // A move to where it already is restarts nothing.
    expect(stepDrag(fast, move(880, 400, 100))).toBe(fast)
    // Before the heading ages out, a still changes nothing.
    expect(stepDrag(fast, { kind: "still", t: 30 + restAfter - 1, targets })).toBe(fast)
  })

  it("keeps as much of the path as its heading reads, and no more", () => {
    // Samples 10ms apart: the heading reads the last 100ms of them.
    const moves = Array.from({ length: 30 }, (_, index) =>
      move(600 + index * 10, 400, index * 10),
    )
    const carried = run([press(), ready, ...moves])
    if (carried.kind !== "carrying") throw new Error("not carrying")
    expect(carried.path.at(-1)?.t).toBe(290)
    expect(carried.path[0]?.t).toBe(190)
  })

  it("release while the page shows a zone that offers something drops it there", () => {
    expect(run([release()], carrying())).toEqual({
      kind: "dropping",
      carried: pane,
      aim: middleOf2,
    })
  })

  it("release commits the zone shown, never one decided again as the button lifts", () => {
    // Heading right fast into pane 2: the page shows "right of".
    const fast = run([
      press(),
      ready,
      move(90, 40, 0),
      move(800, 400, 10),
      move(840, 400, 20),
      move(880, 400, 30),
    ])
    const right: Aim = { target: 2, zone: "right", within: "right" }
    expect(fast).toMatchObject({ aim: right })
    // Let go just after the heading aged out, before the rest was shown:
    // what was on the page — "right of" — is what drops, not the middle
    // the release point would settle to now.
    expect(run([release(right, 880, 400, targets, 30 + restAfter + 5)], fast)).toEqual({
      kind: "dropping",
      carried: pane,
      aim: right,
    })
    // Let go before the next zone was drawn: the one on the page drops.
    expect(run([move(1090, 400, 40), release(middleOf2, 1090)], carrying())).toEqual({
      kind: "dropping",
      carried: pane,
      aim: middleOf2,
    })
  })

  it("release with nothing shown, or where nothing can be aimed at, goes home", () => {
    const home = { kind: "cancelling", how: "home" }
    // A flick: lifted and let go over a zone before any preview was shown.
    expect(
      run([press(), ready, move(90, 40, 10), move(827, 400, 12), release(null)]),
    ).toEqual(home)
    // Off the grid once its preview has cleared; the carried pane's own
    // place, or a side the room refuses: shown as nothing.
    expect(run([move(827, 900, 500), release(null, 827, 900)], carrying())).toEqual(home)
    expect(run([release(null)], carrying())).toEqual(home)
    // Let go off the grid, out of the window or over a side column before
    // the frame cleared what was shown: nothing there can be aimed at.
    expect(run([release(middleOf2, 1300, 400)], carrying())).toEqual(home)
    const peeked = { ...targets, covered: [{ left: 0, top: 0, width: 256, height: 800 }] }
    expect(run([release(middleOf2, 100, 400, peeked)], carrying(peeked))).toEqual(home)
  })

  it("another pointer's release changes nothing", () => {
    const carried = carrying()
    expect(stepDrag(carried, { ...release(), pointerId: 2 } as Event)).toBe(carried)
  })

  it("Escape, another button, or a lost pointer goes home — and the press goes with it", () => {
    for (const end of [
      { kind: "escape" },
      move(830, 400, 500, targets, 3),
      move(830, 400, 500, targets, 0),
      { kind: "lost" },
    ] as const) {
      const after = run([end], carrying())
      expect(after).toEqual({ kind: "cancelling", how: "home" })
      // Lost, then a move and a release over a zone.
      const later = run([move(827, 400, 600), release(), { kind: "landed" }], after)
      expect(later).toBe(idle)
      expect(run([move(827, 400, 700), release()], later)).toBe(idle)
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

  it("carries an item the same way", () => {
    expect(
      run([press(item), ready, move(90, 40, 10), move(827, 400, 20), release()]),
    ).toMatchObject({ kind: "dropping", carried: item, aim: { target: 2 } })
  })
})

describe("dropping and cancelling", () => {
  it("the drop's own change, a press, or any other event changes nothing until it lands", () => {
    const dropping = run([release()], carrying())
    const home = run([{ kind: "escape" }], carrying())
    for (const phase of [dropping, home])
      for (const event of [
        { kind: "changed" },
        press(),
        ready,
        move(100, 100, 600),
        release(),
        { kind: "escape" },
        { kind: "lost" },
        { kind: "still", t: 900, targets },
      ] as const)
        expect(stepDrag(phase, event)).toBe(phase)
  })

  it("its flight landing ends it", () => {
    expect(run([release(), { kind: "landed" }], carrying())).toBe(idle)
    expect(run([{ kind: "changed" }, { kind: "landed" }], carrying())).toBe(idle)
  })
})

describe("what the copy is drawn at", () => {
  const own = { width: 546, height: 800 }
  it("with a zone shown, the slot it would land in: a tall pane over a wide one's lower half is wide and short", () => {
    expect(copyShape(own, { width: 1100, height: 396 })).toEqual({
      width: 1100,
      height: 396,
    })
  })

  it("with no zone shown — off the grid, a side column, a refused side — its own size", () => {
    expect(copyShape(own, null)).toBe(own)
  })
})
