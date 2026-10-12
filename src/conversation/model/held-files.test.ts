import { describe, expect, it } from "vitest"
import { placePublished, type PublishedFile } from "./held-files"

const file = (name: string, executionId?: string): PublishedFile => ({
  name,
  digest: `sha256:${"a".repeat(64)}`,
  mimeType: "text/plain",
  size: 1,
  ...(executionId === undefined ? {} : { executionId }),
})

describe("placePublished", () => {
  it("puts each file under the turn that published it, in the order recorded", () => {
    const placed = placePublished(
      [file("a", "one"), file("b", "two"), file("c", "one")],
      ["one", "two"],
    )
    expect([...placed.byExecution]).toEqual([
      ["one", [file("a", "one"), file("c", "one")]],
      ["two", [file("b", "two")]],
    ])
    expect(placed.before).toEqual([])
    expect(placed.after).toEqual([])
  })

  it("puts a file from a turn the view left out before the first turn shown", () => {
    const placed = placePublished([file("old", "gone"), file("new", "one")], ["one"])
    expect(placed.before).toEqual([file("old", "gone")])
    expect(placed.byExecution.get("one")).toEqual([file("new", "one")])
  })

  it("puts a file published while no turn ran after the last turn", () => {
    const placed = placePublished([file("loose"), file("old", "gone")], ["one"])
    expect(placed.after).toEqual([file("loose")])
    expect(placed.before).toEqual([file("old", "gone")])
  })

  it("drops nothing when no turn is shown at all", () => {
    const placed = placePublished([file("old", "gone"), file("loose")], [])
    expect(placed.byExecution.size).toBe(0)
    expect([...placed.before, ...placed.after]).toHaveLength(2)
  })
})
