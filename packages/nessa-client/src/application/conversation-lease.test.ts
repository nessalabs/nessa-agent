import { describe, expect, it } from "vitest"
import type { ConversationLease } from "../generated/product.js"
import { conversationLeaseStatus } from "./conversation-lease.js"

describe("a lease's status in words", () => {
  it("says whether it runs, why it ended, or why it could not start", () => {
    const cases: [Pick<ConversationLease, "state" | "cause" | "refusal">, string][] = [
      [{ state: "live" }, "Allowed to run"],
      [{ state: "ending", cause: "closed" }, "Stopping"],
      [{ state: "ended", cause: "closed" }, "Closed"],
      [{ state: "ended", cause: "stopped" }, "Stopped"],
      [{ state: "ended" }, "Ended"],
      [{ state: "ended", cause: "revoked" }, "Access withdrawn"],
      [{ state: "ended", cause: "expired" }, "Timed out"],
      [{ state: "ended", cause: "lost" }, "Ended when Nessa restarted"],
      [{ state: "interrupted", cause: "stopped" }, "Cleanup not confirmed"],
      [
        { state: "refused", refusal: "sandbox_unavailable" },
        "Couldn't start: sandbox not available",
      ],
      [{ state: "refused" }, "Couldn't start"],
      [{ state: "unreadable" }, "Not known"],
    ]
    for (const [lease, status] of cases)
      expect(conversationLeaseStatus(lease)).toBe(status)
  })
})
