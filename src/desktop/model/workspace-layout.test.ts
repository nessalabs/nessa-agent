import { describe, expect, it } from "vitest"
import {
  defaultWorkspaceLayout,
  parseWorkspaceLayout,
  workspaceLayouts,
} from "./workspace-layout"

describe("workspace layouts", () => {
  it("reads every layout it lists", () => {
    for (const layout of workspaceLayouts)
      expect(parseWorkspaceLayout(layout.id)).toBe(layout.id)
  })

  it("falls back to three columns for anything else", () => {
    expect(defaultWorkspaceLayout).toBe("columns")
    for (const value of [null, undefined, "", "grid", "toString", 3])
      expect(parseWorkspaceLayout(value)).toBe("columns")
  })
})
