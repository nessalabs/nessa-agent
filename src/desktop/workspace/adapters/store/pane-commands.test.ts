/** The pane commands: drafts, the room, widgets, and what a drag commits. */
import { describe, expect, it } from "vitest"

import { panesOf } from "../../../split-panes/model/pane-layout"

import { fakeSource, ready, settle, shown, testStore } from "../../testing"

import {
  canNudge,
  canOpenBeside,
  closePane,
  closeSessionTab,
  dropSession,
  fitPanes,
  loadWorkspace,
  movePane,
  newSession,
  nudgePane,
  openBeside,
  openChannel,
  openSession,
  resizePanes,
  selectChannel,
  showContent,
  focusPane,
  equalizePanes,
  toggleSidebar,
  closeInFront,
  commitDrop,
  openWidget,
} from "./commands"
import { workspaceSplitPanes } from "./split-panes-source"
import { paneItemKey, sessionItem, widgetItem } from "../../model/pane-item"

describe("new sessions and closing", () => {
  it("starts a draft under a fresh id, and closes the focused pane by default", async () => {
    const { store } = await ready()
    store.dispatch(newSession({ beside: "right" }))
    const draftId = shown(store)[1]
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
    store.dispatch(closePane())
    expect(shown(store)).toEqual(["a"])
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
  })

  it("opens a channel with no sessions on a new session's home", async () => {
    const { store } = await ready()
    store.dispatch(openChannel({ channelId: "empty" }))
    const [shownId] = shown(store)
    expect(store.getState().workspace.drafts[shownId].channelId).toBe("empty")
  })
})

describe("the room every change of layout is held to", () => {
  const widths = (store: ReturnType<typeof testStore>, gridWidth: number) => {
    const columns = store.getState().workspace.panes!.columns
    const total = columns.reduce((sum, column) => sum + column.share, 0)
    return columns.map(
      (column) => ((gridWidth - (columns.length - 1) * 8) * column.share) / total,
    )
  }

  it("holds an agent's split to the room the page measures, as it does a key's", async () => {
    const narrow = { width: 500, height: 800, spare: 0 }
    const store = testStore(fakeSource(), () => narrow)
    await store.dispatch(loadWorkspace())
    await settle()
    // No column fits: a split to the right is refused, whoever asks.
    expect(store.dispatch(newSession({ beside: "right" }))).toBeUndefined()
    expect(shown(store)).toEqual(["a"])
    // "Beside" with no side named stacks, where a row fits.
    store.dispatch(openBeside({ sessionId: "c" }))
    expect(store.getState().workspace.panes!.columns.map((c) => c.panes.length)).toEqual([
      2,
    ])
  })

  it("measures the room for an agent's move and drop, as a person's drag reads it", async () => {
    let measured = 0
    const store = testStore(fakeSource(), () => {
      measured++
      return { width: 1100, height: 800, spare: 0 }
    })
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    const [first, second] = panesOf(store.getState().workspace.panes!).map((p) => p.key)
    // A side needs the room: given none, the agent's command measures it.
    const before = measured
    store.dispatch(movePane({ pane: first, target: second, zone: "bottom" }))
    expect(measured).toBe(before + 1)
    expect(store.getState().workspace.panes!.columns.map((c) => c.panes.length)).toEqual([
      2,
    ])
    store.dispatch(dropSession({ sessionId: "b", target: first, zone: "right" }))
    expect(measured).toBe(before + 2)
    expect(shown(store)).toContain("b")
    expect(store.getState().workspace.panes!.columns).toHaveLength(2)
  })

  it("moves nothing to a side for an agent when nothing was measured", async () => {
    let room: { width: number; height: number; spare: number } | undefined = {
      width: 1100,
      height: 800,
      spare: 0,
    }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    const layout = store.getState().workspace.panes
    const [first, second] = panesOf(layout!).map((p) => p.key)
    room = undefined
    store.dispatch(movePane({ pane: first, target: second, zone: "bottom" }))
    expect(store.getState().workspace.panes).toBe(layout)
  })

  it("places nothing new when nothing was measured", async () => {
    const store = testStore(fakeSource(), () => undefined)
    await store.dispatch(loadWorkspace())
    await settle()
    expect(store.dispatch(newSession({ beside: "right" }))).toBeUndefined()
    expect(store.dispatch(canOpenBeside())).toBe(false)
    // Opening beside with no room takes the focused pane's place, as it says.
    store.dispatch(openBeside({ sessionId: "c" }))
    expect(shown(store)).toEqual(["c"])
  })

  it("moves a pane out into a column of its own only where every pane stays readable", async () => {
    // Two columns of two, as a 1440 × 900 window with both side columns lays them out.
    const room = { width: 840, height: 850, spare: 256 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(newSession({ beside: "bottom" }))
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(newSession({ beside: "bottom" }))
    const first = store.getState().workspace.panes!.columns[0].panes[0].key
    expect(store.dispatch(canNudge({ pane: first, direction: "left" }))).toBe(true)
    store.dispatch(nudgePane({ pane: first, direction: "left" }))
    // Three columns fit only with the sidebar folded, and none is under 300.
    expect(store.getState().workspace.panes!.columns).toHaveLength(3)
    expect(store.getState().workspace.chrome.sidebar.folded).toBe(true)
    expect(Math.min(...widths(store, room.width + room.spare))).toBeGreaterThanOrEqual(
      299.5,
    )
  })

  it("refuses the move, and says so before it is asked, when no fold makes room", async () => {
    const room = { width: 840, height: 850, spare: 0 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(newSession({ beside: "bottom" }))
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(newSession({ beside: "bottom" }))
    const before = store.getState().workspace.panes
    const first = before!.columns[0].panes[0].key
    expect(store.dispatch(canNudge({ pane: first, direction: "left" }))).toBe(false)
    store.dispatch(nudgePane({ pane: first, direction: "left" }))
    expect(store.getState().workspace.panes).toBe(before)
  })

  it("fits the panes again when their room shrinks, rebalancing where it must", async () => {
    let room = { width: 1100, height: 800, spare: 0 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(
      resizePanes({ edge: { axis: "x", column: 0 }, fraction: 0.72, pair: 1092 }),
    )
    room = { width: 700, height: 800, spare: 0 }
    store.dispatch(fitPanes())
    expect(Math.min(...widths(store, room.width))).toBeGreaterThanOrEqual(299.5)
  })
})

describe("a widget over the panes is left as the overview is, but for ⌘W", () => {
  const run = { plugin: "experiments", id: "run" }
  const ways = {
    "Escape or its close": showContent({ content: "panes" }),
    "the channel already listed": selectChannel({ channelId: "desktop" }),
    "a session": openSession({ sessionId: "c" }),
    "a session beside": openBeside({ sessionId: "c" }),
    "a channel from the tree": openChannel({ channelId: "gateway" }),
    "a new session": newSession(),
    "another pane": focusPane({ pane: 1 }),
    "a widget in a pane": openWidget({ widget: run, place: "pane", origin: "a" }),
    "closing a pane": closePane(),
    "closing what is in front (⌘W)": closeInFront(),
    "moving a pane": nudgePane({ pane: 1, direction: "right" }),
    "evening the panes out": equalizePanes(),
    "resizing panes": resizePanes({
      edge: { axis: "x", column: 0 },
      fraction: 0.6,
      pair: 1,
    }),
  }
  for (const [way, action] of Object.entries(ways))
    it(`goes back to the panes on ${way}`, async () => {
      const { store } = await ready()
      store.dispatch(openBeside({ sessionId: "c", side: "right" }))
      store.dispatch(
        resizePanes({ edge: { axis: "x", column: 0 }, fraction: 0.4, pair: 1 }),
      )
      store.dispatch(openWidget({ widget: run, place: "window" }))
      expect(store.getState().workspace.content).toEqual({ widget: run })
      store.dispatch(action as Parameters<typeof store.dispatch>[0])
      expect(store.getState().workspace.content).toBe("panes")
    })

  it("closes on ⌘W, leaving every pane beneath it as it was", async () => {
    const { store } = await ready()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    const panes = store.getState().workspace.panes
    store.dispatch(openWidget({ widget: run, place: "window" }))
    store.dispatch(closeInFront())
    expect(store.getState().workspace.panes).toBe(panes)
    expect(store.getState().workspace.content).toBe("panes")
    // Over the panes, ⌘W is the focused pane's again.
    store.dispatch(closeInFront())
    expect(shown(store)).toEqual(["a"])
  })

  it("closes the focused pane on ⌘W over the overview, as 238 has it", async () => {
    const { store } = await ready()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(closeInFront())
    expect(shown(store)).toEqual(["a"])
    expect(store.getState().workspace.content).toBe("panes")
  })

  it("goes to the overview on ⌘0", async () => {
    const { store } = await ready()
    store.dispatch(openWidget({ widget: run, place: "window" }))
    store.dispatch(showContent({ content: "agents" }))
    expect(store.getState().workspace.content).toBe("agents")
  })

  it("stays when asked for again, and when a change to the panes changes nothing", async () => {
    const { store } = await ready()
    store.dispatch(openWidget({ widget: run, place: "window" }))
    const state = store.getState().workspace
    store.dispatch(
      openWidget({ widget: { plugin: "experiments", id: "run" }, place: "window" }),
    )
    expect(store.getState().workspace).toBe(state)
    const panes = state.panes
    if (!panes) throw new Error("no panes")
    store.dispatch(nudgePane({ pane: panes.focused, direction: "left" }))
    store.dispatch(closePane({ pane: 999 }))
    store.dispatch(toggleSidebar())
    expect(store.getState().workspace.panes).toBe(panes)
    expect(store.getState().workspace.content).toEqual({ widget: run })
  })

  it("replaces the widget it shows with another opened there", async () => {
    const { store } = await ready()
    const other = { plugin: "subagents", id: "a" }
    store.dispatch(openWidget({ widget: run, place: "window" }))
    store.dispatch(openWidget({ widget: other, place: "window" }))
    expect(store.getState().workspace.content).toEqual({ widget: other })
  })

  it("is aimed at by no drag, as the overview is not", async () => {
    const { store } = await ready()
    const source = workspaceSplitPanes(store)
    expect(source.targetable()).toBe(true)
    store.dispatch(openWidget({ widget: run, place: "window" }))
    expect(source.targetable()).toBe(false)
  })
})

describe("a widget in a pane, as a person or an agent opens one", () => {
  const run = { plugin: "experiments", id: "run" }

  it("opens beside its origin's pane, in the room the page measures", async () => {
    const { store } = await ready()
    store.dispatch(openWidget({ widget: run, place: "pane", origin: "a" }))
    expect(shown(store)).toEqual(["a", "widget experiments/run"])
  })

  it("is in a pane and over the panes at once", async () => {
    const { store } = await ready()
    store.dispatch(openWidget({ widget: run, place: "pane", origin: "a" }))
    store.dispatch(openWidget({ widget: run, place: "window" }))
    expect(shown(store)).toEqual(["a", "widget experiments/run"])
    expect(store.getState().workspace.content).toEqual({ widget: run })
  })
})

describe("what a drag carries into the grid", () => {
  it("holds a listed session's row by its pane item, and nothing else", async () => {
    const { store } = await ready()
    const source = workspaceSplitPanes(store)
    expect(source.holds(paneItemKey(sessionItem("d")))).toBe(true)
    expect(source.holds(paneItemKey(sessionItem("missing")))).toBe(false)
    expect(source.holds(paneItemKey(sessionItem("constructor")))).toBe(false)
    expect(source.holds(paneItemKey(widgetItem({ plugin: "p", id: "d" })))).toBe(false)
    // A bare session id is no pane item: nothing reads it as one.
    expect(source.holds("d")).toBe(false)
  })

  it("drops a session's row, and nothing that is not one", async () => {
    const { store } = await ready()
    const target = store.getState().workspace.panes!.focused
    const room = { width: 1100, height: 800, spare: 0 }
    store.dispatch(
      commitDrop({
        carried: {
          kind: "item",
          item: paneItemKey(widgetItem({ plugin: "p", id: "d" })),
        },
        target,
        zone: "center",
        room,
      }),
    )
    store.dispatch(
      commitDrop({ carried: { kind: "item", item: "d" }, target, zone: "center", room }),
    )
    expect(shown(store)).toEqual(["a"])
    store.dispatch(
      commitDrop({
        carried: { kind: "item", item: paneItemKey(sessionItem("d")) },
        target,
        zone: "center",
        room,
      }),
    )
    expect(shown(store)).toEqual(["d"])
  })
})

it("closing a background document leaves the Agents view selected", async () => {
  const { store } = await ready()
  store.dispatch(openSession({ sessionId: "c" }))
  store.dispatch(showContent({ content: "agents" }))
  store.dispatch(closeSessionTab({ sessionId: "c" }))
  expect(store.getState().workspace.content).toBe("agents")
  expect(store.getState().workspace.sessionTabs).not.toContain("c")
})
