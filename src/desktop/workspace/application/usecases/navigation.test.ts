import { describe, expect, it } from "vitest"
import { focusedPane, panesOf } from "../../model/pane-layout"
import { sessionListLimits, sidebarLimits } from "../../model/window-fit"
import { testOrganisation } from "../../testing"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import {
  fitToWindow,
  openChannel,
  resizeSessionList,
  resizeSidebar,
  revealSession,
  selectChannel,
  selectStatusView,
  toggleChannel,
  toggleSection,
  toggleSessionList,
  toggleShowAll,
  toggleSidebar,
} from "./navigation"
import { organisationLoaded } from "./updates"

const loaded = () =>
  organisationLoaded(initialWorkspace, {
    organisation: testOrganisation(),
    draftId: "unused",
  })

const shown = (state: WorkspaceState) =>
  panesOf(state.panes!).map((pane) => pane.sessionId)

describe("choosing what the list shows", () => {
  it("shows a channel's sessions and leaves the panes alone while the list is open", () => {
    const state = selectChannel(loaded(), { channelId: "gateway" })
    expect(state.view).toEqual({ kind: "channel", channelId: "gateway" })
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

  it("shows every session in one state", () => {
    expect(selectStatusView(loaded(), { status: "running" }).view).toEqual({
      kind: "status",
      status: "running",
    })
  })
})

describe("opening a channel from the sidebar", () => {
  it("opens its newest session in place, and discloses it", () => {
    const state = openChannel(loaded(), { channelId: "gateway" })
    expect(shown(state)).toEqual(["d"])
    expect(state.tree.expandedChannels).toContain("gateway")
  })

  it("opens it beside the focused pane when asked", () => {
    expect(shown(openChannel(loaded(), { channelId: "gateway", beside: true }))).toEqual([
      "a",
      "d",
    ])
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
    expect(state.chrome.sidebarOpen).toBe(true)
    expect(state.tree.collapsedSections).not.toContain("labs")
    expect(state.tree.expandedChannels).toContain("gateway")
    expect(state.view).toEqual({ kind: "channel", channelId: "gateway" })
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
    expect(toggleSidebar(state).chrome.sidebarOpen).toBe(false)
    expect(toggleSidebar(state, { open: true })).toBe(state)
    expect(toggleSessionList(state).chrome.sessionListOpen).toBe(false)
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
    expect(fitted.chrome).toMatchObject({ sidebarOpen: false, sessionListOpen: false })
    const noList = fitToWindow(loaded(), {
      windowWidth: 500,
      sidebarWidth: 264,
      sessionListWidth: 0,
    })
    expect(noList.chrome).toMatchObject({ sidebarOpen: false, sessionListOpen: true })
    const wide = loaded()
    expect(
      fitToWindow(wide, { windowWidth: 1440, sidebarWidth: 240, sessionListWidth: 312 }),
    ).toBe(wide)
  })
})
