import { expect, it } from "vitest"
import { selectedTurnActivity } from "./activity-selection"
import type { AgentTurnContentView } from "./agent-transcript-view"

it("drops selected turn activity when its replacement snapshot removes the segment", () => {
  const selected: AgentTurnContentView = { key: "activity", activity: [], running: false }
  expect(selectedTurnActivity([selected], selected.key)).toBe(selected)
  expect(selectedTurnActivity([], selected.key)).toBeUndefined()
  expect(
    selectedTurnActivity([{ key: selected.key, text: "answer" }], selected.key),
  ).toBeUndefined()
})
