import { describe, expect, it } from "vitest"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { roomyGrid, shownBy, testIndex } from "../../testing"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import { closePanelTab, openWidgetInPanel, showPanelTab } from "./chat-panel"
import { openBeside } from "./panes"
import { indexLoaded, sessionRemoved } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, { index: testIndex(), draftId: "unused", read: "r" })

/** Session ids per column, as drawn. */
const drawn = (state: WorkspaceState) =>
  state.panes?.columns.map((column) => column.panes.map(shownBy))
const focused = (state: WorkspaceState) =>
  shownBy(panesOf(state.panes!).find((pane) => pane.key === state.panes!.focused)!)

const run = { plugin: "experiments", id: "run" }
const notes = { plugin: "sample", id: "notes" }

describe("opening a widget in its chat's panel", () => {
  it("adds a tab to the chat's own panel, in front, asks for the panel, and leaves the panes", () => {
    const before = loaded()
    const state = openWidgetInPanel(before, { widget: run, origin: "a" })
    expect(state.panels).toEqual({ a: { tabs: [run], front: run } })
    expect(state.panelAsked).toBe(before.panelAsked + 1)
    expect(state.panes).toEqual(before.panes)
  })

  it("adds each new widget after the others, and brings one open already to the front, once", () => {
    const two = openWidgetInPanel(openWidgetInPanel(loaded(), { widget: run }), {
      widget: notes,
    })
    expect(two.panels.a).toEqual({ tabs: [run, notes], front: notes })
    const again = openWidgetInPanel(two, { widget: { plugin: "experiments", id: "run" } })
    expect(again.panels.a).toEqual({ tabs: [run, notes], front: run })
    // Asked for again, the panel is asked for again too.
    expect(again.panelAsked).toBe(two.panelAsked + 1)
  })

  it("brings its chat to the front: its pane focused, or shown in the focused pane's place", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomyGrid })
    expect(focused(two)).toBe("c")
    const toA = openWidgetInPanel(two, { widget: run, origin: "a" })
    expect(drawn(toA)).toEqual([["a"], ["c"]])
    expect(focused(toA)).toBe("a")
    const toD = openWidgetInPanel(toA, { widget: notes, origin: "d" })
    expect(drawn(toD)).toEqual([["d"], ["c"]])
    expect(toD.panels).toEqual({
      a: { tabs: [run], front: run },
      d: { tabs: [notes], front: notes },
    })
  })

  it("does nothing for a chat the window does not have, or with no chat in front", () => {
    const state = loaded()
    expect(openWidgetInPanel(state, { widget: run, origin: "missing" })).toBe(state)
    expect(openWidgetInPanel(initialWorkspace, { widget: run })).toBe(initialWorkspace)
  })
})

describe("a chat's panel tabs", () => {
  const open = () =>
    [run, notes].reduce(
      (state, widget) => openWidgetInPanel(state, { widget, origin: "a" }),
      loaded(),
    )

  it("brings a tab to the front, and changes nothing for one it does not have", () => {
    const state = open()
    expect(showPanelTab(state, { sessionId: "a", widget: run }).panels.a.front).toEqual(
      run,
    )
    expect(showPanelTab(state, { sessionId: "a", widget: notes })).toBe(state)
    expect(showPanelTab(state, { sessionId: "b", widget: run })).toBe(state)
  })

  it("closes a tab, the next coming to the front, else the one before; the last takes the panel's entry", () => {
    const front = showPanelTab(open(), { sessionId: "a", widget: run })
    const closed = closePanelTab(front, { sessionId: "a", widget: run })
    expect(closed.panels.a).toEqual({ tabs: [notes], front: notes })
    const back = closePanelTab(open(), { sessionId: "a", widget: notes })
    expect(back.panels.a).toEqual({ tabs: [run], front: run })
    expect(closePanelTab(back, { sessionId: "a", widget: run }).panels).toEqual({})
  })

  it("goes with its chat", () => {
    const state = sessionRemoved(open(), { sessionId: "a", revision: 9, draftId: "x" })
    expect(state.panels).toEqual({})
  })
})
