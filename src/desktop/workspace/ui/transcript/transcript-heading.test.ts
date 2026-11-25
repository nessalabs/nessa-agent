import { describe, expect, it } from "vitest"
import { leaseNote, placeNote } from "./transcript-heading"

describe("the heading's word on the agent's lease", () => {
  it("says where the lease stands in the client's words", () => {
    expect(leaseNote({ state: "live", environment: "here" })).toBe("Allowed to run")
    expect(leaseNote({ state: "ended", cause: "lost" })).toBe(
      "Ended when Nessa restarted",
    )
    expect(leaseNote({ state: "refused", refusal: "sandbox_unavailable" })).toBe(
      "Couldn't start: sandbox not available",
    )
    expect(
      leaseNote({ state: "ended", cause: "lost", environment: "ssh", host: "devbox" }),
    ).toBe("Ended when the connection to its host was lost")
  })

  it("says nothing of a lease the gateway cannot read", () => {
    expect(leaseNote({ state: "unreadable" })).toBeUndefined()
  })
})

describe("the heading's word on where the agent ran", () => {
  it("says nothing before any lease: every agent runs on this machine", () => {
    expect(placeNote(undefined)).toBeUndefined()
    expect(placeNote({ state: "unreadable" })).toBeUndefined()
  })

  it("is where the lease ran, an older build's elsewhere included", () => {
    expect(placeNote({ state: "live", environment: "here" })).toBe("This computer")
    expect(placeNote({ state: "live", environment: "boat" })).toBe("Cloud workspace")
    expect(placeNote({ state: "live", environment: "ssh", host: "devbox" })).toBe(
      "devbox",
    )
    expect(
      placeNote({
        state: "live",
        environment: "peer",
        host: `peer:0123abcd${"ef".repeat(28)}`,
      }),
    ).toBe("Paired machine 0123abcd")
  })
})
