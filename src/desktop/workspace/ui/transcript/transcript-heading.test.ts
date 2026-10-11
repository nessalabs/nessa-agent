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

  it("names the paired machine a lease runs on by the start of its key", () => {
    expect(
      leaseNote({
        state: "live",
        environment: "peer",
        host: `peer:0123abcd${"ef".repeat(28)}`,
      }),
    ).toBe("Allowed to run · Paired machine 0123abcd")
  })

  it("says nothing of a lease the gateway cannot read", () => {
    expect(leaseNote({ state: "unreadable" })).toBeUndefined()
  })
})
