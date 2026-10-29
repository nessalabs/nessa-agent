import { describe, expect, it } from "vitest"
import { dropOutcome } from "../../../split-panes/model/drop"
import {
  focusedPane,
  paneCount,
  panesOf,
  paneLimits,
} from "../../../split-panes/model/pane-layout"
import { astra, roomyGrid, shownBy, summary, testIndex } from "../../testing"
import { paneItemKey, sessionItem } from "../../model/pane-item"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import {
  canClosePane,
  canOpenBeside,
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
import { composerTextChanged } from "./sessions"
import { indexLoaded } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
    read: "r",
  })

/** Session ids per column, as drawn. */
const drawn = (state: WorkspaceState) =>
  state.panes?.columns.map((column) => column.panes.map(shownBy))

const keyOf = (state: WorkspaceState, sessionId: string) =>
  panesOf(state.panes!).find((pane) => shownBy(pane) === sessionId)!.key

const roomy = roomyGrid

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
    expect(shownBy(focusedPane(again.panes!))).toBe("b")
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

  it("does nothing before the index arrives", () => {
    expect(openSession(initialWorkspace, { sessionId: "a" })).toBe(initialWorkspace)
    expect(openBeside(initialWorkspace, { sessionId: "a", room: roomy })).toBe(
      initialWorkspace,
    )
  })
})

describe("opening beside", () => {
  it("opens a column to the right of the focused pane and focuses it", () => {
    const state = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(drawn(state)).toEqual([["a"], ["c"]])
    expect(shownBy(focusedPane(state.panes!))).toBe("c")
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
    // Folded for room, not by the person: their choice stands, and the fold lifts with room.
    expect(state.chrome.sidebar.folded).toBe(true)
    expect(state.chrome.sidebar.open).toBe(true)
  })

  it("takes the target's place when nothing fits, unless told not to", () => {
    const tiny = { width: 400, height: 300, spare: 0 }
    expect(drawn(openBeside(loaded(), { sessionId: "c", room: tiny }))).toEqual([["c"]])
    const state = loaded()
    expect(openBeside(state, { sessionId: "c", room: tiny, replace: false })).toBe(state)
  })

  it("replaces the target in a full workspace, and starts no draft beside one", () => {
    const index = testIndex()
    let state = indexLoaded(initialWorkspace, {
      index: {
        ...index,
        sessions: [...index.sessions, summary("e", "gateway", 50)],
      },
      draftId: "unused",
      read: "r",
    })
    state = openBeside(state, { sessionId: "b", room: roomy })
    state = openBeside(state, { sessionId: "c", side: "bottom", room: roomy })
    state = openBeside(state, {
      sessionId: "d",
      target: keyOf(state, "a"),
      side: "bottom",
      room: roomy,
    })
    expect(paneCount(state.panes!)).toBe(paneLimits.maxPanes)
    expect(createDraft(state, { draftId: "new", beside: "right", room: roomy })).toBe(
      state,
    )
    const replaced = openBeside(state, { sessionId: "e", room: roomy })
    expect(paneCount(replaced.panes!)).toBe(paneLimits.maxPanes)
    expect(shownBy(focusedPane(replaced.panes!))).toBe("e")
    expect(panesOf(replaced.panes!).map(shownBy)).not.toContain("d")
  })
})

describe("dropping a session on a pane", () => {
  it("opens it in place in the middle, and beside on a side", () => {
    const state = loaded()
    const target = keyOf(state, "a")
    expect(
      drawn(dropSession(state, { sessionId: "c", target, zone: "center", room: roomy })),
    ).toEqual([["c"]])
    expect(
      drawn(dropSession(state, { sessionId: "c", target, zone: "left", room: roomy })),
    ).toEqual([["c"], ["a"]])
    expect(
      drawn(dropSession(state, { sessionId: "c", target, zone: "top", room: roomy })),
    ).toEqual([["c", "a"]])
  })

  it("focuses a session already on screen instead of moving it", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const dropped = dropSession(two, {
      sessionId: "c",
      target: keyOf(two, "a"),
      zone: "center",
      room: roomy,
    })
    expect(drawn(dropped)).toEqual([["a"], ["c"]])
    expect(shownBy(focusedPane(dropped.panes!))).toBe("c")
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
    const state = createDraft(loaded(), { draftId: "new", beside: "right", room: roomy })
    expect(drawn(state)).toEqual([["a"], ["new"]])
  })

  it("keeps a draft document when its pane changes or closes", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const replaced = openSession(drafted, { sessionId: "c" })
    expect(replaced.drafts.new).toBeDefined()
    const beside = createDraft(loaded(), { draftId: "new", beside: "right", room: roomy })
    const closed = closePane(beside, { pane: keyOf(beside, "new") })
    expect(closed.drafts.new).toBeDefined()
  })

  it("does not reuse an id already in use", () => {
    const state = loaded()
    expect(createDraft(state, { draftId: "a" })).toBe(state)
  })
})

describe("a draft with something typed in it", () => {
  const typed = () =>
    composerTextChanged(createDraft(loaded(), { draftId: "new" }), {
      sessionId: "new",
      text: "half a thought",
    })

  it("outlives its pane when another chat takes it, and New Session shows it again", () => {
    const away = openSession(typed(), { sessionId: "b" })
    expect(drawn(away)).toEqual([["b"]])
    expect(away.composerText.new).toBe("half a thought")
    const back = createDraft(away, { draftId: "another", restore: true })
    expect(drawn(back)).toEqual([["new"]])
    expect(back.drafts).not.toHaveProperty("another")
    const beside = createDraft(away, {
      draftId: "another",
      beside: "right",
      target: away.panes!.focused,
      room: roomy,
      restore: true,
    })
    expect(drawn(beside)).toEqual([["b"], ["new"]])
  })

  it("is not what a draft asked for without restore gets: that one starts blank", () => {
    const away = openSession(typed(), { sessionId: "b" })
    const blank = createDraft(away, { draftId: "another" })
    expect(drawn(blank)).toEqual([["another"]])
    expect(blank.composerText.new).toBe("half a thought")
  })

  it("stays in its document tab after its pane closes", () => {
    const two = openBeside(typed(), { sessionId: "b", room: roomy })
    const closed = closePane(two, { pane: keyOf(two, "new") })
    expect(closed.drafts.new).toBeDefined()
    expect(closed.composerText.new).toBe("half a thought")
    expect(closed.sessionTabs).toContain("new")
    expect(drawn(openSession(closed, { sessionId: "new" }))).toEqual([["new"]])
  })
})

describe("closing panes", () => {
  it("closes a pane and gives its room to its neighbour", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(drawn(closePane(two, { pane: keyOf(two, "c") }))).toEqual([["a"]])
  })

  it("turns the last pane's conversation back into a new session's home", () => {
    const state = closePane(loaded(), { pane: keyOf(loaded(), "a"), draftId: "fresh" })
    expect(drawn(state)).toEqual([["fresh"]])
    expect(state.drafts.fresh).toMatchObject({ channelId: "desktop" })
    const chosen = { ...loaded(), chosenModels: { a: astra } }
    const closed = closePane(chosen, { pane: keyOf(chosen, "a"), draftId: "fresh" })
    expect(closed.drafts.fresh.model).toEqual(chosen.sessions.a.model)
  })

  it("leaves a last pane that is already a home alone", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    expect(closePane(drafted, { pane: keyOf(drafted, "new"), draftId: "other" })).toBe(
      drafted,
    )
  })

  it("is offered on every pane of several and a last one showing a conversation, not a last home", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(canClosePane(two, keyOf(two, "c"))).toBe(true)
    expect(canClosePane(loaded(), keyOf(loaded(), "a"))).toBe(true)
    const drafted = createDraft(loaded(), { draftId: "new" })
    expect(canClosePane(drafted, keyOf(drafted, "new"))).toBe(false)
  })

  it("ignores a pane it does not have", () => {
    const state = loaded()
    expect(closePane(state, { pane: 999, draftId: "x" })).toBe(state)
  })
})

describe("moving, nudging, resizing", () => {
  it("moves a pane and focuses it", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const moved = movePane(two, {
      pane: keyOf(two, "c"),
      target: keyOf(two, "a"),
      zone: "top",
      room: roomy,
    })
    expect(drawn(moved)).toEqual([["c", "a"]])
  })

  it("asks the sidebar to step aside for a moved pane's new column", () => {
    const stacked = openBeside(loaded(), { sessionId: "c", side: "bottom", room: roomy })
    const moved = movePane(stacked, {
      pane: keyOf(stacked, "c"),
      target: keyOf(stacked, "a"),
      zone: "right",
      room: { width: 500, height: 400, spare: 240 },
    })
    expect(drawn(moved)).toEqual([["a"], ["c"]])
    expect(moved.chrome.sidebar.folded).toBe(true)
  })

  it("refuses a move that would leave a pane unreadable, whoever asks", () => {
    const stacked = openBeside(loaded(), { sessionId: "c", side: "bottom", room: roomy })
    const narrow = { width: 500, height: 400, spare: 0 }
    const refused = movePane(stacked, {
      pane: keyOf(stacked, "c"),
      target: keyOf(stacked, "a"),
      zone: "right",
      room: narrow,
    })
    expect(refused).toBe(stacked)
    const short = { width: 900, height: 300, spare: 0 }
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
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
    expect(movePane(state, { pane: key, target: key, zone: "left", room: roomy })).toBe(
      state,
    )
  })

  it("nudges, resizes and evens out through the layout's rules", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(
      drawn(nudgePane(two, { pane: keyOf(two, "c"), direction: "left", room: roomy })),
    ).toEqual([["c"], ["a"]])
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

describe("what a split promises", () => {
  it("splits right only to the right: where no column fits, nothing, never a silent split down", () => {
    const narrow = { width: 500, height: 800, spare: 0 }
    const state = loaded()
    expect(createDraft(state, { draftId: "new", beside: "right", room: narrow })).toBe(
      state,
    )
    expect(canOpenBeside(state, { side: "right", room: narrow })).toBe(false)
    // Asked for no side, "beside" may stack: a row fits.
    expect(canOpenBeside(state, { room: narrow })).toBe(true)
  })

  it("offers beside nowhere in a full workspace, so no menu promises it", () => {
    let state = loaded()
    for (const [sessionId, side] of [
      ["b", "right"],
      ["c", "bottom"],
      ["d", "bottom"],
    ] as const)
      state = openBeside(state, { sessionId, side, room: roomy })
    expect(paneCount(state.panes!)).toBe(paneLimits.maxPanes)
    expect(canOpenBeside(state, { room: roomy })).toBe(false)
  })
})

describe("a drop commits what its drag previewed", () => {
  const zones = ["left", "right", "top", "bottom", "center"] as const

  it("leaves exactly the previewed layout, for every zone, pane or session", () => {
    const two = openBeside(loaded(), { sessionId: "c", side: "right", room: roomy })
    const [first, second] = panesOf(two.panes!).map((pane) => pane.key)
    for (const zone of zones) {
      const preview = dropOutcome(
        two.panes!,
        { kind: "pane", pane: second },
        first,
        zone,
        roomy,
      )
      const moved = movePane(two, { pane: second, target: first, zone, room: roomy })
      expect(moved.panes).toEqual(preview ? preview.layout : two.panes)
      const shown = dropOutcome(
        two.panes!,
        { kind: "item", item: paneItemKey(sessionItem("d")) },
        first,
        zone,
        roomy,
      )
      const dropped = dropSession(two, {
        sessionId: "d",
        target: first,
        zone,
        room: roomy,
      })
      expect(dropped.panes).toEqual(shown ? shown.layout : two.panes)
    }
  })
})
