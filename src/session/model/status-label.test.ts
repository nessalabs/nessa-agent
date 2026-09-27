/**
 * What an empty conversation says under its avatar, and what it leaves to the
 * notice.
 *
 * Two surfaces describe one connection: this label, and the connection notice
 * that carries the reason and the retry. When the label repeated the reason,
 * a refused credential was printed twice on one short panel — identical
 * sentences, with the button attached to only one of them. The label owns the
 * state; the notice owns the explanation.
 */
import { describe, expect, it } from "vitest"

import { statusLabel, type SessionState } from "./types"

function session(over: Partial<SessionState>): SessionState {
  return {
    retryRequest: 0,
    phase: "idle",
    detail: "",
    hello: null,
    health: null,
    ...over,
  }
}

const refusal =
  "No chat credential has been provisioned yet. Start the local server " +
  "(`just start`, or `just server`), which creates one on first run."

describe("the status label", () => {
  it("names the state rather than repeating the reason the notice carries", () => {
    const label = statusLabel(session({ phase: "error", detail: refusal }))
    expect(label).toBe("Not connected")
    expect(label).not.toContain("just server")
  })

  it("says the same thing whatever the reason was, so the notice stays the one account", () => {
    const one = statusLabel(session({ phase: "error", detail: refusal }))
    const other = statusLabel(
      session({ phase: "error", detail: "Desktop and gateway stages must match" }),
    )
    expect(one).toBe(other)
  })

  it("still distinguishes connected, connecting, and reconnecting", () => {
    expect(statusLabel(session({ phase: "ready" }))).toBe("Connected")
    expect(statusLabel(session({ phase: "connecting" }))).toContain("Connecting")
    expect(statusLabel(session({ phase: "reconnecting" }))).toContain("Reconnecting")
  })
})
