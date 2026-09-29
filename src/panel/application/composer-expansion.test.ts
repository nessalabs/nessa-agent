import { describe, expect, it } from "vitest"
import { takesExpansion } from "./composer-expansion"

describe("takesExpansion", () => {
  /**
   * Submitting is not sending. The gateway may be away, the draft may be over
   * the size the gateway accepts — and the composer makes the same call either
   * way, so its offer to close is never the evidence the pane is finished with.
   * `useComposer` closes it on the draft having actually gone.
   */
  it("never closes the pane on a submit alone", () => {
    expect(takesExpansion(false, true)).toBe(false)
  })

  /** The ways out of the pane, which must keep working. */
  it("closes the pane for every reason a person or the composer gives", () => {
    expect(takesExpansion(false, false)).toBe(true)
  })

  it("opens the pane whatever the reason", () => {
    expect(takesExpansion(true, true)).toBe(true)
    expect(takesExpansion(true, false)).toBe(true)
  })
})
