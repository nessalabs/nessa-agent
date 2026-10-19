import { describe, expect, it } from "vitest"
import { percents } from "../../model/shown-window"
import { roomyGrid, testIndex } from "../../testing"
import { initialWorkspace } from "../workspace-state"
import { showContent, toggleSessionList, toggleSidebar } from "./navigation"
import { createDraft, openBeside, openWidget } from "./panes"
import { indexLoaded } from "./updates"
import { arrangementShown, shownWindow } from "./window"

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

  it("names a widget in a pane and a widget over the panes", () => {
    const widget = { plugin: "files", id: "readme" }
    const inPane = openWidget(inColumns(), { widget, room: roomyGrid })
    expect(
      shownWindow(inPane)?.columns.flatMap((c) => c.panes.map((p) => p.item)),
    ).toContainEqual({ kind: "widget", widget })
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

describe("percents", () => {
  it("are whole shares of their sum, and zero when nothing has room", () => {
    expect(percents([1, 1, 2])).toEqual([25, 25, 50])
    expect(percents([1, 2])).toEqual([33, 67])
    expect(percents([0, 0])).toEqual([0, 0])
  })
})
