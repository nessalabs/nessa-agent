import { describe, expect, it } from "vitest"
import { leaseNote } from "./transcript-heading"

describe("the heading's word on the agent's lease", () => {
  it("says where the lease stands in the client's words", () => {
    expect(leaseNote({ state: "live", environment: "here" })).toBe("Running")
    expect(leaseNote({ state: "ended", cause: "lost" })).toBe(
      "Ended when Nessa restarted",
    )
    expect(leaseNote({ state: "refused", refusal: "sandbox_unavailable" })).toBe(
      "Couldn't start: sandbox not available",
    )
  })

  it("says nothing of a lease the gateway cannot read", () => {
    expect(leaseNote({ state: "unreadable" })).toBeUndefined()
  })
})
