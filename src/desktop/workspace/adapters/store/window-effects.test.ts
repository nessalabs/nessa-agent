import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import { createQuestionFlights } from "../../application/question-flights"
import type { ShownWindow } from "../../model/shown-window"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import {
  arrangementShown,
  loadWorkspace,
  openBeside,
  openWidget,
  panelShown,
  toggleSidebar,
} from "./commands"

function storeTelling(told: (ShownWindow | null)[]) {
  return makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: (() => {
      let next = 0
      return () => `id-${++next}`
    })(),
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    windowPublisher: { publish: (window) => told.push(window) },
  })
}

describe("telling what the window shows", () => {
  it("says each change once, and nothing until a layout of panes is on screen", async () => {
    const told: (ShownWindow | null)[] = []
    const store = storeTelling(told)
    await store.dispatch(loadWorkspace())
    await settle()
    expect(told).toEqual([])
    store.dispatch(arrangementShown({ arrangement: "columns" }))
    expect(told).toHaveLength(1)
    expect(told[0]?.columns).toHaveLength(1)
    store.dispatch(openBeside({ sessionId: "b" }))
    expect(told.at(-1)?.columns).toHaveLength(2)
    const before = told.length
    // The same window said again is not said twice.
    store.dispatch(arrangementShown({ arrangement: "columns" }))
    expect(told).toHaveLength(before)
    store.dispatch(toggleSidebar({ open: false }))
    expect(told.at(-1)?.sidebar).toBe(false)
    // The layout gone: the window shows no panes.
    store.dispatch(arrangementShown({ arrangement: null }))
    expect(told.at(-1)).toBeNull()
  })

  it("says the focused chat's panel when its tabs or whether it is drawn change", async () => {
    const told: (ShownWindow | null)[] = []
    const store = storeTelling(told)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(arrangementShown({ arrangement: "columns" }))
    expect(told.at(-1)?.panel).toEqual({ open: false, tabs: [], front: null })
    store.dispatch(
      openWidget({ widget: { plugin: "subagents", id: "a" }, place: "panel" }),
    )
    expect(told.at(-1)?.panel).toEqual({
      open: false,
      tabs: [{ plugin: "subagents", id: "a" }],
      front: { plugin: "subagents", id: "a" },
    })
    store.dispatch(panelShown({ open: true }))
    expect(told.at(-1)?.panel.open).toBe(true)
    const before = told.length
    store.dispatch(panelShown({ open: true }))
    expect(told).toHaveLength(before)
  })
})
