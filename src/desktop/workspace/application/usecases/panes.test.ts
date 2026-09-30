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
import {
  focusedChannel,
  initialWorkspace,
  onScreen,
  shownIds,
  type WorkspaceState,
} from "../workspace-state"
import { defaultModel } from "../../model/workspace-index"
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
  openWidget,
  resizePanes,
} from "./panes"
import { indexLoaded, sessionRemoved, unreadShown } from "./updates"

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

  it("lets a draft go once no pane shows it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const replaced = openSession(drafted, { sessionId: "c" })
    expect(replaced.drafts).toEqual({})
    const beside = createDraft(loaded(), { draftId: "new", beside: "right", room: roomy })
    const closed = closePane(beside, { pane: keyOf(beside, "new") })
    expect(closed.drafts).toEqual({})
  })

  it("does not reuse an id already in use", () => {
    const state = loaded()
    expect(createDraft(state, { draftId: "a" })).toBe(state)
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

describe("a widget in a pane of its own", () => {
  const run = { plugin: "experiments", id: "run:1" }
  const runShown = "widget experiments/run:1"
  const other = { plugin: "subagents", id: "a" }

  it("opens beside the pane showing the session it belongs to, and focuses it", () => {
    const state = openWidget(loaded(), { widget: run, origin: "a", room: roomy })
    expect(drawn(state)).toEqual([["a"], [runShown]])
    expect(shownBy(focusedPane(state.panes!))).toBe(runShown)
  })

  it("opens beside its origin wherever that pane is, not beside the focused one", () => {
    const two = openBeside(loaded(), { sessionId: "c", side: "bottom", room: roomy })
    const state = openWidget(two, { widget: run, origin: "a", room: roomy })
    expect(drawn(state)).toEqual([["a", "c"], [runShown]])
  })

  it("opens beside its origin's pane when another pane is focused", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(shownBy(focusedPane(two.panes!))).toBe("c")
    const state = openWidget(two, { widget: run, origin: "a", room: roomy })
    expect(drawn(state)).toEqual([["a"], [runShown], ["c"]])
  })

  it("takes its origin's place where nothing fits beside it, as openBeside does", () => {
    const narrow = { width: 500, height: 400, spare: 0 }
    const state = openWidget(loaded(), { widget: run, origin: "a", room: narrow })
    expect(drawn(state)).toEqual([[runShown]])
  })

  it("takes the focused pane's place with no origin, or one no pane shows", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    expect(drawn(openWidget(two, { widget: run, room: roomy }))).toEqual([
      ["a"],
      [runShown],
    ])
    expect(drawn(openWidget(two, { widget: run, origin: "d", room: roomy }))).toEqual([
      ["a"],
      [runShown],
    ])
  })

  it("is shown once: asked again, by an equal reference, its pane is focused where it is", () => {
    const opened = openWidget(loaded(), { widget: run, origin: "a", room: roomy })
    const back = focusPane(opened, keyOf(opened, "a"))
    const again = openWidget(back, {
      widget: { plugin: "experiments", id: "run:1" },
      origin: "a",
      room: roomy,
    })
    expect(drawn(again)).toEqual([["a"], [runShown]])
    expect(shownBy(focusedPane(again.panes!))).toBe(runShown)
  })

  it("never shares a pane with a session whose id reads like its key", () => {
    const lookalike = { plugin: "experiments", id: "a" }
    const state = openWidget(loaded(), { widget: lookalike, origin: "a", room: roomy })
    expect(drawn(state)).toEqual([["a"], ["widget experiments/a"]])
  })

  it("is shown by any plugin, known or not: what is drawn is the host's", () => {
    const state = openWidget(loaded(), { widget: other, room: roomy })
    expect(drawn(state)).toEqual([["widget subagents/a"]])
  })

  it("does nothing before the index arrives", () => {
    expect(openWidget(initialWorkspace, { widget: run, room: roomy })).toBe(
      initialWorkspace,
    )
  })

  it("is never read as the session whose id spells its key", () => {
    // A listed session named as the prototype's key would have named the widget.
    const lookalike = "w:experiments:run%003A1"
    const withIt = {
      ...loaded(),
      sessions: {
        ...loaded().sessions,
        [lookalike]: summary(lookalike, "desktop", 500, "idle", { unread: true }),
      },
    }
    const state = openWidget(withIt, { widget: run, room: roomy })
    expect(shownIds(state).has(lookalike)).toBe(false)
    expect(unreadShown(state)).toEqual([])
    expect(onScreen(state, lookalike)).toBe(false)
    expect(focusedChannel(state)).toBeUndefined()
  })

  it("is not a session: no channel, no conversation read or kept, no draft", () => {
    const before = loaded()
    const state = openWidget(before, { widget: run, origin: "a", room: roomy })
    expect(focusedChannel(state)).toBeUndefined()
    expect([...shownIds(state)]).toEqual(["a"])
    expect(onScreen(state, "a")).toBe(true)
    expect(state.drafts).toEqual({})
    // No channel is disclosed for it, as one is for a session opened.
    expect(state.tree).toBe(before.tree)
  })

  it("gives its place to a session opened, dropped or started in it", () => {
    const shown = openWidget(loaded(), { widget: run, room: roomy })
    expect(drawn(openSession(shown, { sessionId: "c" }))).toEqual([["c"]])
    expect(
      drawn(
        dropSession(shown, {
          sessionId: "d",
          target: keyOf(shown, runShown),
          zone: "center",
          room: roomy,
        }),
      ),
    ).toEqual([["d"]])
    const drafted = createDraft(shown, { draftId: "new" })
    expect(drawn(drafted)).toEqual([["new"]])
    // In the channel being looked at: a widget's pane has none of its own.
    expect(drafted.drafts.new.channelId).toBe("desktop")
  })

  it("moves, nudges and closes as any pane does", () => {
    const two = openWidget(loaded(), { widget: run, origin: "a", room: roomy })
    const moved = movePane(two, {
      pane: keyOf(two, runShown),
      target: keyOf(two, "a"),
      zone: "top",
      room: roomy,
    })
    expect(drawn(moved)).toEqual([[runShown, "a"]])
    const nudged = nudgePane(two, {
      pane: keyOf(two, runShown),
      direction: "left",
      room: roomy,
    })
    expect(drawn(nudged)).toEqual([[runShown], ["a"]])
    expect(drawn(closePane(two, { pane: keyOf(two, runShown) }))).toEqual([["a"]])
  })

  it("stays where it is when a session goes, its origin among them", () => {
    const two = openWidget(loaded(), { widget: run, origin: "a", room: roomy })
    const removed = sessionRemoved(two, { sessionId: "a", revision: 9, draftId: "x" })
    expect(drawn(removed)).toEqual([[runShown]])
    const alone = openWidget(loaded(), { widget: run, room: roomy })
    expect(
      drawn(sessionRemoved(alone, { sessionId: "a", revision: 9, draftId: "x" })),
    ).toEqual([[runShown]])
  })
})

describe("closing the last pane when it shows a widget", () => {
  const run = { plugin: "experiments", id: "run" }

  it("goes back to a new session's home in the channel being looked at, on the default model", () => {
    const shown = openWidget(
      { ...loaded(), chosenModels: { a: astra } },
      { widget: run, room: roomy },
    )
    const closed = closePane(shown, { pane: shown.panes!.focused, draftId: "fresh" })
    expect(drawn(closed)).toEqual([["fresh"]])
    expect(closed.drafts.fresh).toEqual({
      id: "fresh",
      channelId: "desktop",
      model: defaultModel(),
    })
  })

  it("goes to the first channel when the one looked at is gone", () => {
    const shown = openWidget(loaded(), { widget: run, room: roomy })
    const lost = { ...shown, view: { channelId: "nowhere" } }
    const closed = closePane(lost, { pane: lost.panes!.focused, draftId: "fresh" })
    expect(closed.drafts.fresh.channelId).toBe("desktop")
    const elsewhere = { ...shown, view: { channelId: "gateway" } }
    const there = closePane(elsewhere, {
      pane: elsewhere.panes!.focused,
      draftId: "fresh",
    })
    expect(there.drafts.fresh.channelId).toBe("gateway")
  })

  it("is not offered, and changes nothing, where no new session could start", () => {
    const shown = openWidget(loaded(), { widget: run, room: roomy })
    const bare = { ...shown, channels: [], view: { channelId: "" } }
    const pane = bare.panes!.focused
    expect(canClosePane(shown, pane)).toBe(true)
    expect(canClosePane(bare, pane)).toBe(false)
    expect(closePane(bare, { pane, draftId: "fresh" })).toBe(bare)
    // Offered exactly where it does something, whatever the last pane shows.
    const drafted = createDraft(loaded(), { draftId: "home" })
    // A conversation whose channel the window no longer holds starts over where a new session goes.
    const unheld = { ...loaded(), channels: [] }
    for (const state of [loaded(), shown, bare, drafted, unheld]) {
      const last = state.panes!.focused
      const closed = closePane(state, { pane: last, draftId: "fresh" })
      expect(canClosePane(state, last)).toBe(closed !== state)
    }
  })

  it("leaves it as it is without an id for the new session", () => {
    const shown = openWidget(loaded(), { widget: run, room: roomy })
    expect(closePane(shown, { pane: shown.panes!.focused })).toBe(shown)
  })
})
