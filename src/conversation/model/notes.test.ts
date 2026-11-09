import { describe, expect, it } from "vitest"
import { noteAuthorLabel, noteStateLabel, placeNotes, type Note } from "./notes"

const note = (id: string, afterExecutionId?: string): Note => ({
  id,
  text: `Note ${id}`,
  author: { kind: "owner", surface: "desktop" },
  deliveryState: "recorded",
  ...(afterExecutionId === undefined ? {} : { afterExecutionId }),
})

describe("placeNotes", () => {
  it("puts each note right after the turn it followed, in the order left", () => {
    const placed = placeNotes(
      [note("a", "one"), note("b", "two"), note("c", "one")],
      ["one", "two"],
      [],
    )
    expect([...placed.byExecution]).toEqual([
      ["one", [note("a", "one"), note("c", "one")]],
      ["two", [note("b", "two")]],
    ])
    expect(placed.before).toEqual([])
    expect(placed.byWaiting.size).toBe(0)
  })

  it("puts a note left before any turn before the first turn shown", () => {
    const placed = placeNotes([note("first"), note("later", "one")], ["one"], [])
    expect(placed.before).toEqual([note("first")])
    expect(placed.byExecution.get("one")).toEqual([note("later", "one")])
    expect(placed.byWaiting.size).toBe(0)
  })

  it("puts a note whose turn the bounded view left out before the first turn shown, with those left before any turn", () => {
    // The view keeps the newest notes but drops the oldest turns, so a note
    // can follow a turn older than every turn shown.
    const placed = placeNotes(
      [note("first"), note("dropped", "zero"), note("kept", "one")],
      ["one"],
      ["two"],
    )
    expect(placed.before).toEqual([note("first"), note("dropped", "zero")])
    expect(placed.byWaiting.size).toBe(0)
    expect(placed.byExecution.get("one")).toEqual([note("kept", "one")])
  })

  it("puts each note that followed an input still waiting right after that input, not after every input waiting", () => {
    // Two inputs waiting and a note left between them: it belongs to the first.
    const placed = placeNotes(
      [note("between", "q1"), note("b", "one"), note("last", "q2")],
      ["one"],
      ["q1", "q2"],
    )
    expect([...placed.byWaiting]).toEqual([
      ["q1", [note("between", "q1")]],
      ["q2", [note("last", "q2")]],
    ])
    expect(placed.before).toEqual([])
    expect(placed.byExecution.get("one")).toEqual([note("b", "one")])
  })

  it("a turn shown wins over the same turn listed as waiting, and no note is dropped", () => {
    const notes = [note("a", "one"), note("b", "gone"), note("c", "two"), note("d")]
    const placed = placeNotes(notes, ["one"], ["one", "two"])
    expect(placed.byExecution.get("one")).toEqual([note("a", "one")])
    expect(placed.byWaiting.has("one")).toBe(false)
    expect(
      placed.before.length +
        [...placed.byWaiting.values()].flat().length +
        [...placed.byExecution.values()].flat().length,
    ).toBe(notes.length)
  })
})

describe("noteStateLabel", () => {
  it.each([
    [{ deliveryState: "recorded" as const }, null],
    [{ deliveryState: "pending" as const }, "Waiting for the next turn"],
    [{ deliveryState: "assigned" as const }, "Given to a turn"],
    [
      { deliveryState: "retired" as const, retiredReason: "expired" as const },
      "Not delivered: expired",
    ],
    [
      { deliveryState: "retired" as const, retiredReason: "permission_lost" as const },
      "Not delivered: sharing changed",
    ],
    // A reason the view did not give is not guessed at.
    [{ deliveryState: "retired" as const }, "Not delivered"],
  ])("says where %o stands as %s", (state, label) => {
    expect(noteStateLabel(state)).toBe(label)
  })
})

describe("noteAuthorLabel", () => {
  it("names the owner or someone it is shared with, and keeps the surface apart as a name", () => {
    expect(noteAuthorLabel({ kind: "owner", surface: "desktop" })).toEqual([
      "Note left by the owner, from ",
      { name: "desktop" },
    ])
    expect(noteAuthorLabel({ kind: "grant", surface: "phone" })).toEqual([
      "Note left by someone it is shared with, from ",
      { name: "phone" },
    ])
  })
  it("names an agent's source chat apart from its words", () => {
    expect(
      noteAuthorLabel({
        kind: "agent",
        surface: "panel",
        agent: { conversationId: "source-chat", executionId: "source-turn" },
      }),
    ).toEqual([
      "Note left by an agent",
      " in chat ",
      { name: "source-chat" },
      ", from ",
      { name: "panel" },
    ])
  })
})
