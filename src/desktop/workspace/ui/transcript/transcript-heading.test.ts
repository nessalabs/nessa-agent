import { describe, expect, it } from "vitest"
import type { TranscriptLease } from "../../model/transcript"
import { leaseNote } from "./transcript-heading"

describe("the heading's word on where the agent runs", () => {
  it("says the agent runs on this computer while its lease is live", () => {
    expect(leaseNote({ state: "live", environment: "here" })).toBe("On this computer")
    expect(leaseNote({ state: "live" })).toBeUndefined()
  })

  it("says why it stopped, or why it could not start, and nothing for a lease it cannot read", () => {
    const cases: [TranscriptLease, string | undefined][] = [
      [{ state: "ending", cause: "closed" }, "Stopping"],
      [{ state: "ended", cause: "closed" }, "Closed"],
      [{ state: "ended", cause: "stopped" }, "Stopped"],
      [{ state: "ended" }, "Stopped"],
      [{ state: "ended", cause: "revoked" }, "Access withdrawn"],
      [{ state: "ended", cause: "expired" }, "Timed out"],
      [{ state: "ended", cause: "lost" }, "Ended when Nessa restarted"],
      [{ state: "interrupted", cause: "stopped" }, "Cleanup not confirmed"],
      [
        { state: "refused", refusal: "sandbox_unavailable" },
        "Couldn't start: sandbox not available",
      ],
      [{ state: "refused" }, "Couldn't start"],
      [{ state: "unreadable" }, undefined],
    ]
    for (const [lease, note] of cases) expect(leaseNote(lease)).toBe(note)
  })
})
