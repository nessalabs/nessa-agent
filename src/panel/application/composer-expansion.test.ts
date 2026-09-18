import { describe, expect, it } from "vitest"
import { takesExpansion } from "./composer-expansion"

describe("takesExpansion", () => {
  it("closes the pane once a submit has actually sent", () => {
    expect(takesExpansion(false, "submit", true)).toBe(true)
  })

  /** The point of the rule: a draft that never left keeps the pane it was written in. */
  it("keeps the pane when the submit sent nothing", () => {
    expect(takesExpansion(false, "submit", false)).toBe(false)
  })

  /** The ways out of the pane must not be caught by the rule above. */
  it("closes the pane for every reason that is not a submit", () => {
    for (const reason of ["control", "escape", "withdrawal"] as const) {
      expect(takesExpansion(false, reason, false)).toBe(true)
    }
  })

  /** Nothing declines an opening, whatever the last submit did. */
  it("opens the pane whatever the reason", () => {
    for (const reason of ["control", "escape", "submit", "withdrawal"] as const) {
      expect(takesExpansion(true, reason, false)).toBe(true)
    }
  })
})
