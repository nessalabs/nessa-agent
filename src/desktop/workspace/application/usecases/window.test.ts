import { describe, expect, it } from "vitest"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { percents } from "../../model/shown-window"
import { roomyGrid, shownBy, testIndex } from "../../testing"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import { closePanelTab, openWidgetInPanel, showPanelTab } from "./chat-panel"
import { showContent, toggleSessionList, toggleSidebar } from "./navigation"
import { createDraft, focusPane, openBeside } from "./panes"
import { indexLoaded } from "./updates"
import { arrangementShown, panelShown, shownWindow } from "./window"

const loaded = () =>
  indexLoaded(initialWorkspace, { index: testIndex(), draftId: "unused", read: "r" })

const inColumns = () => arrangementShown(loaded(), { arrangement: "columns" })

describe("what the window shows", () => {
  it("is nothing until a layout of panes is on screen", () => {
    expect(shownWindow(loaded())).toBeNull()
    expect(
      shownWindow(arrangementShown(initialWorkspace, { arrangement: "columns" })),
    ).toBeNull()
  })

  it("names each pane left to right with its share and focus", () => {
    const two = openBeside(inColumns(), { sessionId: "b", room: roomyGrid })
    expect(shownWindow(two)).toEqual({
      arrangement: "columns",
      sidebar: true,
      sessionList: true,
      showing: { kind: "panes" },
      panel: { open: false, tabs: [], front: null },
      columns: [
        {
          widthPercent: 50,
          panes: [
            {
              heightPercent: 100,
              focused: false,
              item: { kind: "conversation", sessionId: "a" },
            },
          ],
        },
        {
          widthPercent: 50,
          panes: [
            {
              heightPercent: 100,
              focused: true,
              item: { kind: "conversation", sessionId: "b" },
            },
          ],
        },
      ],
    })
  })

  it("says which side columns are drawn, and no session list in the sidebar layout", () => {
    const hidden = toggleSessionList(toggleSidebar(inColumns(), { open: false }), {
      open: false,
    })
    expect(shownWindow(hidden)).toMatchObject({ sidebar: false, sessionList: false })
    const sidebar = arrangementShown(loaded(), { arrangement: "sidebar" })
    expect(shownWindow(sidebar)).toMatchObject({
      arrangement: "sidebar",
      sessionList: false,
    })
  })

  it("says a session not started yet is a draft", () => {
    const drafted = createDraft(inColumns(), {
      draftId: "new",
      beside: "right",
      room: roomyGrid,
    })
    expect(shownWindow(drafted)?.columns[1].panes[0].item).toEqual({ kind: "draft" })
  })

  it("names a widget over the panes", () => {
    const widget = { plugin: "files", id: "readme" }
    const over = showContent(inColumns(), { content: { widget } })
    expect(shownWindow(over)?.showing).toEqual({ kind: "widget", widget })
    expect(shownWindow(showContent(inColumns(), { content: "agents" }))?.showing).toEqual(
      {
        kind: "agents",
      },
    )
  })

  it("leaves the state as it was when the same layout is said again", () => {
    const state = inColumns()
    expect(arrangementShown(state, { arrangement: "columns" })).toBe(state)
  })
})

const subagents = { plugin: "subagents", id: "chat" }
const app = { plugin: "mcp:huddle", id: "call-1" }
const notes = { plugin: "subagents", id: "other" }

const focused = (state: WorkspaceState) =>
  shownBy(panesOf(state.panes!).find((pane) => pane.key === state.panes!.focused)!)

describe("the focused chat's panel", () => {
  const withTabs = () =>
    panelShown(
      openWidgetInPanel(openWidgetInPanel(inColumns(), { widget: subagents }), {
        widget: app,
      }),
      { open: true },
    )

  it("is closed with no tabs until the layout says otherwise", () => {
    const shown = inColumns()
    expect(shownWindow(shown)?.panel).toEqual({ open: false, tabs: [], front: null })
    expect(panelShown(shown, { open: false })).toBe(shown)
  })

  it("lists the focused chat's tabs while the panel is closed", () => {
    const closed = panelShown(
      openWidgetInPanel(openWidgetInPanel(inColumns(), { widget: subagents }), {
        widget: app,
      }),
      { open: false },
    )
    expect(shownWindow(closed)?.panel).toEqual({
      open: false,
      tabs: [subagents, app],
      front: app,
    })
  })

  it("says the panel is open with those tabs, and which one is in front", () => {
    expect(shownWindow(withTabs())?.panel).toEqual({
      open: true,
      tabs: [subagents, app],
      front: app,
    })
    const brought = showPanelTab(withTabs(), { sessionId: "a", widget: subagents })
    expect(shownWindow(brought)?.panel).toEqual({
      open: true,
      tabs: [subagents, app],
      front: subagents,
    })
  })

  it("follows the chat in front and keeps whether the panel is drawn", () => {
    const beside = openBeside(withTabs(), { sessionId: "b", room: roomyGrid })
    expect(focused(beside)).toBe("b")
    expect(shownWindow(beside)?.panel).toEqual({ open: true, tabs: [], front: null })
    const theirs = openWidgetInPanel(beside, { widget: notes, origin: "b" })
    expect(shownWindow(theirs)?.panel).toEqual({
      open: true,
      tabs: [notes],
      front: notes,
    })
    const back = focusPane(
      theirs,
      panesOf(theirs.panes!).find((pane) => shownBy(pane) === "a")!.key,
    )
    expect(shownWindow(back)?.panel).toEqual({
      open: true,
      tabs: [subagents, app],
      front: app,
    })
  })

  it("keeps the panel open and names no front once the last tab closes", () => {
    const one = closePanelTab(withTabs(), { sessionId: "a", widget: subagents })
    expect(shownWindow(one)?.panel).toEqual({ open: true, tabs: [app], front: app })
    const none = closePanelTab(one, { sessionId: "a", widget: app })
    expect(shownWindow(none)?.panel).toEqual({ open: true, tabs: [], front: null })
  })

  it("stays beside the overview and a widget over the panes", () => {
    const open = withTabs()
    expect(shownWindow(showContent(open, { content: "agents" }))?.panel.open).toBe(true)
    expect(
      shownWindow(showContent(open, { content: { widget: notes } }))?.showing,
    ).toEqual({ kind: "widget", widget: notes })
    expect(shownWindow(showContent(open, { content: { widget: notes } }))?.panel).toEqual(
      shownWindow(open)?.panel,
    )
  })

  it("says a draft in front has no tabs", () => {
    const drafted = createDraft(withTabs(), {
      draftId: "new",
      beside: "right",
      room: roomyGrid,
    })
    expect(focused(drafted)).toBe("new")
    expect(shownWindow(drafted)?.panel).toEqual({ open: true, tabs: [], front: null })
  })
})

describe("percents", () => {
  it("are whole shares of their sum, and zero when nothing has room", () => {
    expect(percents([1, 1, 2])).toEqual([25, 25, 50])
    expect(percents([1, 2])).toEqual([33, 67])
    expect(percents([0, 0])).toEqual([0, 0])
  })
})
