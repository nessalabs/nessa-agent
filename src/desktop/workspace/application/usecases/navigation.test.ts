import { describe, expect, it } from "vitest"
import { focusedPane, panesOf } from "../../../split-panes/model/pane-layout"
import { sessionListLimits, sidebarLimits } from "../../model/window-fit"
import { roomyGrid, testIndex } from "../../testing"
import { drawnColumns, initialWorkspace, type WorkspaceState } from "../workspace-state"
import {
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
import { closePane, openBeside } from "./panes"
import { indexLoaded } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
    read: "r",
  })

const shown = (state: WorkspaceState) =>
  panesOf(state.panes!).map((pane) => pane.sessionId)

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
    expect(focusedPane(state.panes!).sessionId).toBe("new")
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
