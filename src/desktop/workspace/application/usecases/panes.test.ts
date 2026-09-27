import { describe, expect, it } from "vitest"
import { focusedPane, paneCount, panesOf, paneLimits } from "../../model/pane-layout"
import { astra, summary, testOrganisation } from "../../testing"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import {
  closePane,
  createDraft,
  dropSession,
  equalizePanes,
  focusPane,
  movePane,
  nudgePane,
  openBeside,
  openSession,
  resizePanes,
} from "./panes"
import { organisationLoaded } from "./updates"

const loaded = () =>
  organisationLoaded(initialWorkspace, {
    organisation: testOrganisation(),
    draftId: "unused",
  })

/** Session ids per column, as drawn. */
const drawn = (state: WorkspaceState) =>
  state.panes?.columns.map((column) => column.panes.map((pane) => pane.sessionId))

const keyOf = (state: WorkspaceState, sessionId: string) =>
  panesOf(state.panes!).find((pane) => pane.sessionId === sessionId)!.key

const roomy = { width: 1100, height: 800, spare: 0 }

describe("opening a session", () => {
  it("shows it in the focused pane, leaving its read mark to the effect that owns it", () => {
    const state = openSession(loaded(), { sessionId: "b" })
    expect(drawn(state)).toEqual([["b"]])
    expect(state.sessions.b.unread).toBe(true)
  })

  it("focuses the pane already showing it rather than showing it twice", () => {
    const two = openBeside(loaded(), { sessionId: "b", room: roomy })
    const back = focusPane(two, keyOf(two, "a"))
    const again = openSession(back, { sessionId: "b" })
    expect(drawn(again)).toEqual([["a"], ["b"]])
    expect(focusedPane(again.panes!).sessionId).toBe("b")
  })

  it("discloses the session's channel in the sidebar", () => {
    const state = openSession(loaded(), { sessionId: "d" })
    expect(state.tree.expandedChannels).toContain("gateway")
  })

  it("ignores a session it does not have, whatever its name", () => {
    const state = loaded()
    expect(openSession(state, { sessionId: "missing" })).toBe(state)
    expect(openSession(state, { sessionId: "constructor" })).toBe(state)
    expect(openSession(state, { sessionId: "a", pane: 999 })).toBe(state)
  })

  it("does nothing before the organisation arrives", () => {
    expect(openSession(initialWorkspace, { sessionId: "a" })).toBe(initialWorkspace)
    expect(openBeside(initialWorkspace, { sessionId: "a" })).toBe(initialWorkspace)
  })
})

describe("opening beside", () => {
  it("opens a column to the right of the focused pane and focuses it", () => {
    const state = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(drawn(state)).toEqual([["a"], ["c"]])
    expect(focusedPane(state.panes!).sessionId).toBe("c")
  })

  it("stacks instead when a column would not be readable", () => {
    const state = openBeside(loaded(), {
      sessionId: "c",
      room: { width: 500, height: 800, spare: 0 },
    })
    expect(drawn(state)).toEqual([["a", "c"]])
  })

  it("asks the sidebar to step aside when that is what makes the column fit", () => {
    const state = openBeside(loaded(), {
      sessionId: "c",
      room: { width: 500, height: 800, spare: 240 },
    })
    expect(drawn(state)).toEqual([["a"], ["c"]])
    expect(state.chrome.sidebarOpen).toBe(false)
  })

  it("takes the target's place when nothing fits, unless told not to", () => {
    const tiny = { width: 400, height: 300, spare: 0 }
    expect(drawn(openBeside(loaded(), { sessionId: "c", room: tiny }))).toEqual([["c"]])
    const state = loaded()
    expect(openBeside(state, { sessionId: "c", room: tiny, replace: false })).toBe(state)
  })

  it("replaces the target in a full workspace, and starts no draft beside one", () => {
    const organisation = testOrganisation()
    let state = organisationLoaded(initialWorkspace, {
      organisation: {
        ...organisation,
        sessions: [...organisation.sessions, summary("e", "gateway", 50)],
      },
      draftId: "unused",
    })
    state = openBeside(state, { sessionId: "b" })
    state = openBeside(state, { sessionId: "c", side: "bottom" })
    state = openBeside(state, {
      sessionId: "d",
      target: keyOf(state, "a"),
      side: "bottom",
    })
    expect(paneCount(state.panes!)).toBe(paneLimits.maxPanes)
    expect(createDraft(state, { draftId: "new", beside: "right" })).toBe(state)
    const replaced = openBeside(state, { sessionId: "e" })
    expect(paneCount(replaced.panes!)).toBe(paneLimits.maxPanes)
    expect(focusedPane(replaced.panes!).sessionId).toBe("e")
    expect(panesOf(replaced.panes!).map((pane) => pane.sessionId)).not.toContain("d")
  })
})

describe("dropping a session on a pane", () => {
  it("opens it in place in the middle, and beside on a side", () => {
    const state = loaded()
    const target = keyOf(state, "a")
    expect(drawn(dropSession(state, { sessionId: "c", target, zone: "center" }))).toEqual(
      [["c"]],
    )
    expect(
      drawn(dropSession(state, { sessionId: "c", target, zone: "left", room: roomy })),
    ).toEqual([["c"], ["a"]])
    expect(
      drawn(dropSession(state, { sessionId: "c", target, zone: "top", room: roomy })),
    ).toEqual([["c", "a"]])
  })

  it("focuses a session already on screen instead of moving it", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    const dropped = dropSession(two, {
      sessionId: "c",
      target: keyOf(two, "a"),
      zone: "center",
    })
    expect(drawn(dropped)).toEqual([["a"], ["c"]])
    expect(focusedPane(dropped.panes!).sessionId).toBe("c")
  })
})

describe("new sessions", () => {
  it("starts a draft in the focused pane, in the channel being looked at", () => {
    const state = createDraft(loaded(), { draftId: "new" })
    expect(drawn(state)).toEqual([["new"]])
    expect(state.drafts.new).toMatchObject({ id: "new", channelId: "desktop" })
    expect(Object.keys(state.sessions)).not.toContain("new")
  })

  it("starts one beside, without covering a conversation when the workspace is full", () => {
    const state = createDraft(loaded(), { draftId: "new", beside: "right" })
    expect(drawn(state)).toEqual([["a"], ["new"]])
  })

  it("lets a draft go once no pane shows it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const replaced = openSession(drafted, { sessionId: "c" })
    expect(replaced.drafts).toEqual({})
    const beside = createDraft(loaded(), { draftId: "new", beside: "right" })
    const closed = closePane(beside, { pane: keyOf(beside, "new") })
    expect(closed.drafts).toEqual({})
  })

  it("switches a status view to the draft's channel", () => {
    const viewing = { ...loaded(), view: { kind: "status", status: "running" } as const }
    const state = createDraft(viewing, { draftId: "new", channelId: "gateway" })
    expect(state.view).toEqual({ kind: "channel", channelId: "gateway" })
  })

  it("does not reuse an id already in use", () => {
    const state = loaded()
    expect(createDraft(state, { draftId: "a" })).toBe(state)
  })
})

describe("closing panes", () => {
  it("closes a pane and gives its room to its neighbour", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    expect(drawn(closePane(two, { pane: keyOf(two, "c") }))).toEqual([["a"]])
  })

  it("turns the last pane's conversation back into a new session's home", () => {
    const state = closePane(loaded(), { pane: keyOf(loaded(), "a"), draftId: "fresh" })
    expect(drawn(state)).toEqual([["fresh"]])
    expect(state.drafts.fresh).toMatchObject({ channelId: "desktop" })
    const chosen = { ...loaded(), chosenModels: { a: astra } }
    const closed = closePane(chosen, { pane: keyOf(chosen, "a"), draftId: "fresh" })
    expect(closed.drafts.fresh.model).toEqual(astra)
  })

  it("leaves a last pane that is already a home alone", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    expect(closePane(drafted, { pane: keyOf(drafted, "new"), draftId: "other" })).toBe(
      drafted,
    )
  })

  it("ignores a pane it does not have", () => {
    const state = loaded()
    expect(closePane(state, { pane: 999, draftId: "x" })).toBe(state)
  })
})

describe("moving, nudging, resizing", () => {
  it("moves a pane and focuses it", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    const moved = movePane(two, {
      pane: keyOf(two, "c"),
      target: keyOf(two, "a"),
      zone: "top",
    })
    expect(drawn(moved)).toEqual([["c", "a"]])
  })

  it("asks the sidebar to step aside for a moved pane's new column", () => {
    const stacked = openBeside(loaded(), { sessionId: "c", side: "bottom" })
    const moved = movePane(stacked, {
      pane: keyOf(stacked, "c"),
      target: keyOf(stacked, "a"),
      zone: "right",
      room: { width: 500, height: 400, spare: 240 },
    })
    expect(drawn(moved)).toEqual([["a"], ["c"]])
    expect(moved.chrome.sidebarOpen).toBe(false)
  })

  it("refuses a move that would leave a pane unreadable, whoever asks", () => {
    const stacked = openBeside(loaded(), { sessionId: "c", side: "bottom" })
    const narrow = { width: 500, height: 400, spare: 0 }
    const refused = movePane(stacked, {
      pane: keyOf(stacked, "c"),
      target: keyOf(stacked, "a"),
      zone: "right",
      room: narrow,
    })
    expect(refused).toBe(stacked)
    const short = { width: 900, height: 300, spare: 0 }
    const two = openBeside(loaded(), { sessionId: "c" })
    expect(
      movePane(two, {
        pane: keyOf(two, "c"),
        target: keyOf(two, "a"),
        zone: "top",
        room: short,
      }),
    ).toBe(two)
  })

  it("does nothing for a pane dropped on itself", () => {
    const state = loaded()
    const key = keyOf(state, "a")
    expect(movePane(state, { pane: key, target: key, zone: "left" })).toBe(state)
  })

  it("nudges, resizes and evens out through the layout's rules", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    expect(drawn(nudgePane(two, { pane: keyOf(two, "c"), direction: "left" }))).toEqual([
      ["c"],
      ["a"],
    ])
    const resized = resizePanes(two, {
      edge: { axis: "x", column: 0 },
      fraction: 0.7,
      pair: 1000,
    })
    expect(resized.panes!.columns[0].share).toBeCloseTo(0.7)
    expect(equalizePanes(resized).panes!.columns.map((column) => column.share)).toEqual([
      1, 1,
    ])
  })
})
