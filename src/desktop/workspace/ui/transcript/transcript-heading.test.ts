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

describe("the heading's word on where the agent runs", () => {
  it("is the channel's placement before any lease", () => {
    expect(placeNote(undefined, "boat")).toBe("Boat workspace")
    expect(placeNote(undefined, "local")).toBe("This computer")
    expect(placeNote(undefined, undefined)).toBeUndefined()
  })

  it("is where the lease ran once there is one", () => {
    expect(placeNote({ state: "live", environment: "here" }, "boat")).toBe(
      "This computer",
    )
    expect(placeNote({ state: "live", environment: "boat" }, "local")).toBe(
      "Boat workspace",
    )
    expect(
      placeNote({ state: "live", environment: "ssh", host: "devbox" }, undefined),
    ).toBe("devbox")
    expect(
      placeNote(
        { state: "live", environment: "peer", host: `peer:0123abcd${"ef".repeat(28)}` },
        undefined,
      ),
    ).toBe("Paired machine 0123abcd")
  })

  it("falls back to the channel when the lease cannot be read", () => {
    expect(placeNote({ state: "unreadable" }, "boat")).toBe("Boat workspace")
  })
})
