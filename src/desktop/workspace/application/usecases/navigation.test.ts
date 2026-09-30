import { describe, expect, it } from "vitest"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import { sessionListLimits, sidebarLimits } from "../../model/window-fit"
import { roomyGrid, shownBy, shownIn, testIndex } from "../../testing"
import {
  drawnColumns,
  initialWorkspace,
  overviewShows,
  sameContent,
  type WorkspaceState,
} from "../workspace-state"
import {
  closeInFront,
  fitToWindow,
  navigated,
  openChannel,
  resizeSessionList,
  resizeSidebar,
  revealSession,
  selectChannel,
  showContent,
  toggleChannel,
  toggleSection,
  toggleSessionList,
  toggleShowAll,
  toggleSidebar,
} from "./navigation"
import { closePane, openBeside, openWidget } from "./panes"
import { indexLoaded } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
    read: "r",
  })

const shown = (state: WorkspaceState) => shownIn(state.panes)

describe("choosing what the list shows", () => {
  it("shows a channel's sessions and leaves the panes alone while the list is open", () => {
    const state = selectChannel(loaded(), { channelId: "gateway" })
    expect(state.view).toEqual({ channelId: "gateway" })
    expect(shown(state)).toEqual(["a"])
  })

  it("opens the channel's most pressing session when the list is hidden", () => {
    const hidden = toggleSessionList(loaded(), { open: false })
    expect(shown(selectChannel(hidden, { channelId: "desktop" }))).toEqual(["b"])
    expect(shown(selectChannel(hidden, { channelId: "empty" }))).toEqual(["a"])
  })

  it("ignores a channel it does not have", () => {
    const state = loaded()
    expect(selectChannel(state, { channelId: "toString" })).toBe(state)
    expect(openChannel(state, { channelId: "missing", draftId: "x" })).toBe(state)
  })
})

describe("opening a channel from the sidebar", () => {
  it("opens its newest session in place, and discloses it", () => {
    const state = openChannel(loaded(), { channelId: "gateway" })
    expect(shown(state)).toEqual(["d"])
    expect(state.tree.expandedChannels).toContain("gateway")
  })

  it("opens it beside the focused pane when asked", () => {
    expect(
      shown(
        openChannel(loaded(), { channelId: "gateway", beside: true, room: roomyGrid }),
      ),
    ).toEqual(["a", "d"])
  })

  it("stacks beside the focused pane when a column would not be readable", () => {
    const narrow = { width: 500, height: 800, spare: 0 }
    const state = openChannel(loaded(), {
      channelId: "gateway",
      beside: true,
      room: narrow,
    })
    expect(state.panes?.columns.map((column) => column.panes.length)).toEqual([2])
  })

  it("starts a new session in a channel with none", () => {
    const state = openChannel(loaded(), { channelId: "empty", draftId: "new" })
    expect(shown(state)).toEqual(["new"])
    expect(state.drafts.new.channelId).toBe("empty")
    expect(shownBy(focusedPane(state.panes!))).toBe("new")
  })
})

describe("revealing a session", () => {
  it("opens the sidebar and unfolds the session's section and channel", () => {
    let state = toggleSidebar(loaded(), { open: false })
    state = toggleSection(state, { sectionId: "labs" })
    state = revealSession(state, { sessionId: "d" })
    expect(state.chrome.sidebar.open).toBe(true)
    expect(state.tree.collapsedSections).not.toContain("labs")
    expect(state.tree.expandedChannels).toContain("gateway")
    expect(state.view).toEqual({ channelId: "gateway" })
  })

  it("ignores a session it does not have", () => {
    const state = loaded()
    expect(revealSession(state, { sessionId: "missing" })).toBe(state)
  })
})

describe("folding the sidebar's parts", () => {
  it("folds and unfolds sections, channels and their full lists", () => {
    let state = toggleSection(loaded(), { sectionId: "labs" })
    expect(state.tree.collapsedSections).toEqual(["labs"])
    state = toggleSection(state, { sectionId: "labs" })
    expect(state.tree.collapsedSections).toEqual([])
    state = toggleChannel(state, { channelId: "gateway" })
    expect(state.tree.expandedChannels).toContain("gateway")
    expect(toggleChannel(state, { channelId: "gateway", open: true })).toBe(state)
    state = toggleShowAll(state, { channelId: "desktop" })
    expect(state.tree.showAllChannels).toEqual(["desktop"])
  })
})

describe("the side columns", () => {
  it("toggles each column, and does nothing when asked for what it is", () => {
    const state = loaded()
    expect(toggleSidebar(state).chrome.sidebar.open).toBe(false)
    expect(toggleSidebar(state, { open: true })).toBe(state)
    expect(toggleSessionList(state).chrome.sessionList.open).toBe(false)
  })

  it("keeps dragged widths within the columns' limits", () => {
    expect(resizeSidebar(loaded(), { width: 5 }).chrome.sidebarWidth).toBe(
      sidebarLimits.min,
    )
    expect(resizeSessionList(loaded(), { width: 5000 }).chrome.sessionListWidth).toBe(
      sessionListLimits.max,
    )
  })

  it("folds what a narrow window cannot afford, and leaves a list the layout lacks alone", () => {
    const fitted = fitToWindow(loaded(), {
      windowWidth: 600,
      sidebarWidth: 240,
      sessionListWidth: 312,
    })
    // Folded for room: the person's choice stands; only what is drawn changes.
    expect(fitted.chrome).toMatchObject({
      sidebar: { open: true, folded: true },
      sessionList: { open: true, folded: true },
    })
    expect(drawnColumns(fitted.chrome)).toEqual({ sidebar: false, sessionList: false })
    const noList = fitToWindow(loaded(), {
      windowWidth: 500,
      sidebarWidth: 264,
      sessionListWidth: 0,
    })
    expect([noList.chrome.sidebar.folded, noList.chrome.sessionList.folded]).toEqual([
      true,
      false,
    ])
    const wide = loaded()
    expect(
      fitToWindow(wide, { windowWidth: 1440, sidebarWidth: 240, sessionListWidth: 312 }),
    ).toBe(wide)
  })

  it("brings a column it folded back once the window has room again", () => {
    const narrow = fitToWindow(loaded(), {
      windowWidth: 600,
      sidebarWidth: 240,
      sessionListWidth: 312,
    })
    const wide = fitToWindow(narrow, {
      windowWidth: 1440,
      sidebarWidth: 240,
      sessionListWidth: 312,
    })
    expect(drawnColumns(wide.chrome)).toEqual({ sidebar: true, sessionList: true })
    expect([wide.chrome.sidebar.folded, wide.chrome.sessionList.folded]).toEqual([
      false,
      false,
    ])
  })

  it("never brings back a column the person hid", () => {
    const hidden = toggleSidebar(loaded())
    const wide = fitToWindow(hidden, {
      windowWidth: 1440,
      sidebarWidth: 240,
      sessionListWidth: 312,
    })
    expect(drawnColumns(wide.chrome).sidebar).toBe(false)
    expect(wide).toBe(hidden)
  })

  it("lets the person show a folded column at once, their choice winning", () => {
    const narrow = fitToWindow(loaded(), {
      windowWidth: 600,
      sidebarWidth: 240,
      sessionListWidth: 312,
    })
    const shown = toggleSidebar(narrow)
    expect(drawnColumns(shown.chrome).sidebar).toBe(true)
    expect(shown.chrome.sidebar.folded).toBe(false)
  })

  it("brings a sidebar folded for a split back when the column closes", () => {
    const split = openBeside(loaded(), {
      sessionId: "c",
      room: { width: 500, height: 800, spare: 260 },
    })
    expect(split.chrome.sidebar.folded).toBe(true)
    const closed = closePane(split, { pane: split.panes!.focused })
    // The layout fits the window again as the columns change (`useFitOnResize`).
    const fitted = fitToWindow(closed, {
      windowWidth: 900,
      sidebarWidth: 240,
      sessionListWidth: 0,
    })
    expect(drawnColumns(fitted.chrome).sidebar).toBe(true)
  })
})

describe("what fills the content region", () => {
  it("goes to the Agents overview, and asked again stays there: a place, not a switch", () => {
    const agents = showContent(loaded(), { content: "agents" })
    expect(agents.content).toBe("agents")
    expect(showContent(agents, { content: "agents" })).toBe(agents)
  })

  it("goes back to the panes on going anywhere else, and changes nothing already there", () => {
    const agents = showContent(loaded(), { content: "agents" })
    expect(navigated(agents).content).toBe("panes")
    const panes = loaded()
    expect(navigated(panes)).toBe(panes)
  })
})

describe("a widget over the panes: the window place", () => {
  const run = { plugin: "experiments", id: "run" }
  const over = (state: WorkspaceState, widget = run) =>
    showContent(state, { content: { widget } })

  it("covers the panes, which stay beneath it as they were", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomyGrid })
    const shownOver = over(two)
    expect(shownOver.content).toEqual({ widget: run })
    expect(shownOver.panes).toBe(two.panes)
    expect(overviewShows(shownOver, "b")).toBe(false)
  })

  it("stays when the widget shown is asked for again, compared by value", () => {
    const shownOver = over(loaded())
    expect(over(shownOver, { plugin: "experiments", id: "run" })).toBe(shownOver)
    expect(showContent(shownOver, { content: shownOver.content })).toBe(shownOver)
  })

  it("replaces the widget shown with another asked for", () => {
    const other = { plugin: "experiments", id: "other" }
    expect(over(over(loaded()), other).content).toEqual({ widget: other })
    const plugin = { plugin: "subagents", id: "run" }
    expect(over(over(loaded()), plugin).content).toEqual({ widget: plugin })
  })

  it("holds a widget that is in a pane too", () => {
    const inPane = openWidget(loaded(), { widget: run, origin: "a", room: roomyGrid })
    const both = over(inPane)
    expect(shown(both)).toEqual(["a", "widget experiments/run"])
    expect(both.content).toEqual({ widget: run })
  })

  it("is left for the panes by Escape or its close, and by going anywhere else", () => {
    expect(showContent(over(loaded()), { content: "panes" }).content).toBe("panes")
    expect(navigated(over(loaded())).content).toBe("panes")
  })

  it("gives way to the overview on ⌘0, and the overview to it", () => {
    const agents = showContent(over(loaded()), { content: "agents" })
    expect(agents.content).toBe("agents")
    expect(over(agents).content).toEqual({ widget: run })
  })
})

describe("the same place in the content region", () => {
  const run = { plugin: "p", id: "i" }
  it("is the same view, and a widget the same by plugin and id", () => {
    expect(sameContent("panes", "panes")).toBe(true)
    expect(sameContent("panes", "agents")).toBe(false)
    expect(sameContent("agents", { widget: run })).toBe(false)
    expect(sameContent({ widget: run }, "panes")).toBe(false)
    expect(sameContent({ widget: run }, { widget: { plugin: "p", id: "i" } })).toBe(true)
    expect(sameContent({ widget: run }, { widget: { plugin: "p", id: "j" } })).toBe(false)
    expect(sameContent({ widget: run }, { widget: { plugin: "q", id: "i" } })).toBe(false)
  })
})

describe("closing what is in front (⌘W)", () => {
  const run = { plugin: "experiments", id: "run" }

  it("closes a widget over the panes, and never a pane beneath it", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomyGrid })
    const shownOver = showContent(two, { content: { widget: run } })
    const closed = closeInFront(shownOver, { draftId: "fresh" })
    expect(closed.content).toBe("panes")
    expect(closed.panes).toBe(two.panes)
    expect(closed.drafts).toEqual({})
  })

  it("closes the focused pane over the panes, and over the overview", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomyGrid })
    expect(shown(closeInFront(two, { draftId: "fresh" }))).toEqual(["a"])
    const agents = showContent(two, { content: "agents" })
    expect(shown(closeInFront(agents, { draftId: "fresh" }))).toEqual(["a"])
  })

  it("turns the last pane back into a new session's home, as closing it does", () => {
    const closed = closeInFront(loaded(), { draftId: "fresh" })
    expect(shown(closed)).toEqual(["fresh"])
  })

  it("does nothing before the index arrives", () => {
    expect(closeInFront(initialWorkspace, { draftId: "fresh" })).toBe(initialWorkspace)
  })
})
