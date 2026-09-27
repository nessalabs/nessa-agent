import { expect, it } from "vitest"
import { selectedWork } from "./work-selection"

it("drops selected working when its replacement snapshot removes the segment", () => {
  const selected = { key: "work", work: [{ key: "step" }] }
  expect(selectedWork([selected], selected.key)).toBe(selected)
  expect(selectedWork([], selected.key)).toBeUndefined()
  expect(selectedWork([{ key: selected.key }], selected.key)).toBeUndefined()
})

it("keeps the same working selected as the turn grows a step", () => {
  const key = "turn:work"
  // The segment is keyed on the turn, so an open sheet survives the snapshot.
  expect(
    selectedWork([{ key, work: [{ key: "a" }, { key: "b" }] }], key)?.work,
  ).toHaveLength(2)
})
