import { describe, expect, it } from "vitest"

import { setupRecovery } from "./setup-recovery"
import type { SetupHandoff } from "../../host"

describe("what is left on screen when setup is over", () => {
  it("leaves nothing behind when the window is closing", () => {
    expect(setupRecovery({ outcome: "handed-over" })).toBeNull()
  })

  it("leaves nothing behind where there was never a second window", () => {
    expect(setupRecovery({ outcome: "no-native-host" })).toBeNull()
  })

  it("shows nothing while the handoff has not answered", () => {
    expect(setupRecovery(undefined)).toBeNull()
  })

  it("offers the handoff again when the panel did not come up", () => {
    const recovery = setupRecovery({
      outcome: "panel-unavailable",
      cause: new Error("there is no panel to summon"),
    })
    expect(recovery?.offer).toBe("hand-over-again")
    expect(recovery?.heading).toBe("Nessa could not open the panel")
  })

  // The defect: a close that failed after a summon that worked was reported as
  // a summon that failed, so somebody was told Nessa could not open the panel
  // while the panel stood open behind that sentence.
  it("says the panel is up when only this window would not close", () => {
    const recovery = setupRecovery({
      outcome: "setup-close-failed",
      panelShown: true,
      cause: "could not close setup: the window server said no",
    })
    expect(recovery?.offer).toBeNull()
    expect(recovery?.heading).not.toContain("could not open the panel")
    expect(recovery?.detail).toContain("the panel is ready")
  })

  // The other defect at the same seam, and the worse one: the panel came up,
  // the write was refused, and the window closed itself anyway — so nothing
  // ever said that the agent just chosen had not been kept, and every
  // conversation of that launch ran on the gateway's default.
  it("offers the write again when the panel is up and nothing was recorded", () => {
    const recovery = setupRecovery({
      outcome: "setup-not-recorded",
      cause: "could not record that setup finished: disk full",
    })
    expect(recovery?.offer).toBe("save-again")
    expect(recovery?.heading).not.toContain("could not open the panel")
    // Both halves of the cost. Somebody who closes this window instead has
    // lost the choice, and is owed the chance to know that.
    expect(recovery?.detail).toContain("ask again next time")
    expect(recovery?.detail).toContain("default agent")
  })

  it("treats an ending it does not recognize as a panel that did not come up", () => {
    // Both ways out are on that screen, so it is the safe thing to be wrong
    // with: it offers a retry as well as a close.
    const unknown = { outcome: "from-a-later-build" } as unknown as SetupHandoff
    expect(setupRecovery(unknown)?.offer).toBe("hand-over-again")
  })
})
