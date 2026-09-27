import { describe, expect, it } from "vitest"
import {
  edgePeekHidden,
  stepEdgePeek,
  type EdgePeek,
  type EdgePeekEvent,
} from "./edge-peek"

const run = (...events: EdgePeekEvent[]): EdgePeek =>
  events.reduce(stepEdgePeek, edgePeekHidden)
const shown: EdgePeek = { shown: true, pending: null, handedOff: false }
const revealed = ["enter", "reveal-due"] as const

describe("stepEdgePeek", () => {
  it("reveals only after resting on the edge", () => {
    expect(run("enter")).toEqual({ shown: false, pending: "reveal", handedOff: false })
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
    expect(run(...revealed, "leave")).toEqual({ ...shown, pending: "hide" })
    expect(run(...revealed, "leave", "hide-due")).toEqual(edgePeekHidden)
  })

  it("hands off to the docked sidebar without hiding first", () => {
    expect(run(...revealed, "dock")).toEqual({ ...shown, pending: "handoff" })
    expect(run(...revealed, "dock", "handoff-due")).toEqual({
      shown: false,
      pending: null,
      handedOff: true,
    })
  })

  it("keeps the handoff whatever the pointer does meanwhile", () => {
    expect(run(...revealed, "dock", "leave")).toEqual({ ...shown, pending: "handoff" })
    expect(run(...revealed, "dock", "leave", "hide-due")).toEqual({
      ...shown,
      pending: "handoff",
    })
  })

  it("docks a pending hide too, and cancels a pending reveal", () => {
    expect(run(...revealed, "leave", "dock")).toEqual({ ...shown, pending: "handoff" })
    expect(run("enter", "dock")).toEqual(edgePeekHidden)
  })

  it("animates normally again after a handoff", () => {
    expect(run(...revealed, "dock", "handoff-due", "enter")).toEqual({
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
    ] as const)
      expect(run(...events, "dismiss")).toEqual(edgePeekHidden)
  })
})
