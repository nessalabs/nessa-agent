import { describe, expect, it } from "vitest"
import {
  edgePeekHidden,
  stepEdgePeek,
  type EdgePeek,
  type EdgePeekEvent,
} from "./edge-peek"

const run = (...events: EdgePeekEvent[]): EdgePeek =>
  events.reduce(stepEdgePeek, edgePeekHidden)
const shown: EdgePeek = { ...edgePeekHidden, shown: true, inside: true }
const revealed = ["enter", "reveal-due"] as const
/** What is shown, and what is on its way: the part of the state a person sees. */
const seen = ({ shown, pending }: EdgePeek) => ({ shown, pending })

describe("stepEdgePeek", () => {
  it("reveals only after resting on the edge", () => {
    expect(seen(run("enter"))).toEqual({ shown: false, pending: "reveal" })
    expect(run(...revealed)).toEqual(shown)
  })

  it("ignores a pass-through", () => {
    expect(run("enter", "leave")).toEqual(edgePeekHidden)
    expect(run("enter", "leave", "reveal-due")).toEqual(edgePeekHidden)
  })

  it("stays while the pointer is on the sidebar", () => {
    expect(run(...revealed, "leave", "enter")).toEqual(shown)
    expect(run(...revealed, "leave", "enter", "hide-due")).toEqual(shown)
  })

  it("hides after the pointer has been away", () => {
    expect(seen(run(...revealed, "leave"))).toEqual({ shown: true, pending: "hide" })
    expect(run(...revealed, "leave", "hide-due")).toEqual(edgePeekHidden)
  })

  it("hands off to the docked sidebar without hiding first", () => {
    expect(seen(run(...revealed, "dock"))).toEqual({ shown: true, pending: "handoff" })
    expect(run(...revealed, "dock", "handoff-due")).toMatchObject({
      shown: false,
      pending: null,
      handedOff: true,
    })
  })

  it("keeps the handoff whatever the pointer does meanwhile", () => {
    expect(seen(run(...revealed, "dock", "leave"))).toEqual({
      shown: true,
      pending: "handoff",
    })
    expect(seen(run(...revealed, "dock", "leave", "hide-due"))).toEqual({
      shown: true,
      pending: "handoff",
    })
  })

  it("docks a pending hide too, and cancels a pending reveal", () => {
    expect(seen(run(...revealed, "leave", "dock"))).toEqual({
      shown: true,
      pending: "handoff",
    })
    expect(seen(run("enter", "dock"))).toEqual({ shown: false, pending: null })
  })

  it("animates normally again after a handoff", () => {
    expect(run(...revealed, "dock", "handoff-due", "enter")).toMatchObject({
      shown: false,
      pending: "reveal",
      handedOff: false,
    })
  })

  it("dismisses from any state", () => {
    for (const events of [
      ["enter"],
      [...revealed],
      [...revealed, "leave"],
      [...revealed, "dock"],
      [...revealed, "press"],
    ] as const)
      expect(seen(run(...events, "dismiss"))).toEqual({ shown: false, pending: null })
  })
})

describe("a press freezes the peek", () => {
  it("cancels a hide on its way: pressed just after leaving, it stays shown however long the press", () => {
    const pressed = run(...revealed, "leave", "press")
    expect(seen(pressed)).toEqual({ shown: true, pending: null })
    // The hide's timer, if it still fires, is stale.
    expect(seen(stepEdgePeek(pressed, "hide-due"))).toEqual({
      shown: true,
      pending: null,
    })
  })

  it("cancels a reveal on its way: pressed while resting at the edge, it stays hidden", () => {
    const pressed = run("enter", "press")
    expect(seen(pressed)).toEqual({ shown: false, pending: null })
    expect(seen(stepEdgePeek(pressed, "reveal-due"))).toEqual({
      shown: false,
      pending: null,
    })
  })

  it("does not show or hide for the pointer entering or leaving while pressed", () => {
    expect(seen(run(...revealed, "press", "leave"))).toEqual({
      shown: true,
      pending: null,
    })
    expect(seen(run("press", "enter"))).toEqual({ shown: false, pending: null })
    expect(run("press", "enter").inside).toBe(true)
  })

  it("at the release, hides when the pointer is away from it", () => {
    const released = run(...revealed, "press", "leave", "release")
    expect(seen(released)).toEqual({ shown: true, pending: "hide" })
    expect(seen(stepEdgePeek(released, "hide-due"))).toEqual({
      shown: false,
      pending: null,
    })
  })

  it("at the release, stays when the pointer is over it", () => {
    expect(run(...revealed, "press", "leave", "enter", "release")).toEqual(shown)
  })

  it("at the release, reveals when the pointer rests at the edge", () => {
    expect(seen(run("press", "enter", "release"))).toEqual({
      shown: false,
      pending: "reveal",
    })
  })

  it("takes a release with no press as nothing", () => {
    const at = run(...revealed, "leave")
    expect(stepEdgePeek(at, "release")).toBe(at)
  })

  it("takes a second press as nothing", () => {
    const at = run(...revealed, "press")
    expect(stepEdgePeek(at, "press")).toBe(at)
  })

  it("lets a handoff go on through a press and its release", () => {
    const docking = run(...revealed, "dock", "press")
    expect(seen(docking)).toEqual({ shown: true, pending: "handoff" })
    expect(seen(stepEdgePeek(docking, "release"))).toEqual({
      shown: true,
      pending: "handoff",
    })
    expect(run(...revealed, "dock", "press", "handoff-due").handedOff).toBe(true)
  })
})
