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
    )
    expect([...placed.byExecution]).toEqual([
      ["one", [note("a", "one"), note("c", "one")]],
      ["two", [note("b", "two")]],
    ])
    expect(placed.before).toEqual([])
    expect(placed.after).toEqual([])
  })

  it("puts a note left before any turn before the first turn shown", () => {
    const placed = placeNotes([note("first"), note("later", "one")], ["one"])
    expect(placed.before).toEqual([note("first")])
    expect(placed.byExecution.get("one")).toEqual([note("later", "one")])
    expect(placed.after).toEqual([])
  })

  it("puts a note whose turn is not shown after the last turn shown, and drops none", () => {
    const notes = [note("a", "unlisted"), note("b", "one"), note("c", "also-unlisted")]
    const placed = placeNotes(notes, ["one"])
    expect(placed.after).toEqual([note("a", "unlisted"), note("c", "also-unlisted")])
    expect(
      placed.before.length +
        placed.after.length +
        [...placed.byExecution.values()].flat().length,
    ).toBe(notes.length)
  })

  it("with no turn shown, a note left before any turn is first and every other is after", () => {
    const placed = placeNotes([note("a"), note("b", "one")], [])
    expect(placed.before).toEqual([note("a")])
    expect(placed.after).toEqual([note("b", "one")])
    expect(placed.byExecution.size).toBe(0)
  })
})

describe("noteStateLabel", () => {
  it.each([
    [{ deliveryState: "recorded" as const }, null],
    [{ deliveryState: "pending" as const }, "Waiting for the next turn"],
    [{ deliveryState: "assigned" as const }, "Given to the next turn"],
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
})
