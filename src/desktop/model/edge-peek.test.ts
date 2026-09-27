import { describe, expect, it } from "vitest"
import {
  edgePeekHidden,
  stepEdgePeek,
  type EdgePeek,
  type EdgePeekEvent,
} from "./edge-peek"

const run = (...events: EdgePeekEvent[]): EdgePeek =>
  events.reduce(stepEdgePeek, edgePeekHidden)
const shown: EdgePeek = { shown: true, pending: null }

describe("stepEdgePeek", () => {
  it("reveals only after resting on the edge", () => {
    expect(run("enter")).toEqual({ shown: false, pending: "reveal" })
    expect(run("enter", "reveal-due")).toEqual(shown)
  })

  it("ignores a pass-through", () => {
    expect(run("enter", "leave")).toEqual(edgePeekHidden)
    expect(run("enter", "leave", "reveal-due")).toEqual(edgePeekHidden)
  })

  it("stays while the pointer is on the sidebar", () => {
    expect(run("enter", "reveal-due", "leave", "enter")).toEqual(shown)
    expect(run("enter", "reveal-due", "leave", "enter", "hide-due")).toEqual(shown)
  })

  it("hides after the pointer has been away", () => {
    expect(run("enter", "reveal-due", "leave")).toEqual({ shown: true, pending: "hide" })
    expect(run("enter", "reveal-due", "leave", "hide-due")).toEqual(edgePeekHidden)
  })

  it("dismisses from any state", () => {
    for (const events of [
      ["enter"],
      ["enter", "reveal-due"],
      ["enter", "reveal-due", "leave"],
    ] as const)
      expect(run(...events, "dismiss")).toEqual(edgePeekHidden)
  })
})
