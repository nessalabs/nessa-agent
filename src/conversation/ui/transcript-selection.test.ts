import { expect, it } from "vitest"
import { selectedToolActivity } from "./tool-selection"

it("drops a selected tool activity when its replacement snapshot removes the segment", () => {
  const selected = { key: "tool-segment", tools: [{ callId: "tool" }] }
  expect(selectedToolActivity([selected], selected.key)).toBe(selected)
  expect(selectedToolActivity([], selected.key)).toBeUndefined()
  expect(selectedToolActivity([{ key: selected.key }], selected.key)).toBeUndefined()
})
