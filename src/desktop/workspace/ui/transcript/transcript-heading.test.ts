import { describe, expect, it } from "vitest"
import { leaseNote } from "./transcript-heading"

describe("the heading's word on the agent's lease", () => {
  it("says where the lease stands in the client's words", () => {
    expect(leaseNote({ state: "live", environment: "here" })).toBe("Allowed to run")
    expect(leaseNote({ state: "ended", cause: "lost" })).toBe(
      "Ended when Nessa restarted",
    )
    expect(leaseNote({ state: "refused", refusal: "sandbox_unavailable" })).toBe(
      "Couldn't start: sandbox not available",
    )
  })

  it("names the SSH host a lease runs on", () => {
    expect(leaseNote({ state: "live", environment: "ssh", host: "devbox" })).toBe(
      "Allowed to run · devbox",
    )
    expect(
      leaseNote({ state: "ended", cause: "lost", environment: "ssh", host: "devbox" }),
    ).toBe("Ended when the connection to its host was lost · devbox")
  })

  it("says nothing of a lease the gateway cannot read", () => {
    expect(leaseNote({ state: "unreadable" })).toBeUndefined()
  })
})
