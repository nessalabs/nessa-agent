import { describe, expect, it } from "vitest"
import type { ConversationLease } from "../generated/product.js"
import { conversationLeasePlace, conversationLeaseStatus } from "./conversation-lease.js"

describe("a lease's status in words", () => {
  it("says whether it runs, why it ended, or why it could not start", () => {
    const cases: [
      Pick<ConversationLease, "state" | "cause" | "refusal" | "environment">,
      string,
    ][] = [
      [{ state: "live" }, "Allowed to run"],
      [{ state: "ending", cause: "closed" }, "Stopping"],
      [{ state: "ended", cause: "closed" }, "Closed"],
      [{ state: "ended", cause: "stopped" }, "Stopped"],
      [{ state: "ended" }, "Ended"],
      [{ state: "ended", cause: "revoked" }, "Access withdrawn"],
      [{ state: "ended", cause: "expired" }, "Timed out"],
      [{ state: "ended", cause: "lost" }, "Ended when Nessa restarted"],
      [
        { state: "ended", cause: "lost", environment: "ssh" },
        "Ended when the connection to its host was lost",
      ],
      [{ state: "interrupted", cause: "stopped" }, "Cleanup not confirmed"],
      [
        { state: "refused", refusal: "sandbox_unavailable" },
        "Couldn't start: sandbox not available",
      ],
      [
        { state: "refused", refusal: "environment_unreachable" },
        "Couldn't start: host not reachable",
      ],
      [
        { state: "refused", refusal: "environment_version_mismatch" },
        "Couldn't start: host runs another version of Nessa",
      ],
      [
        { state: "refused", refusal: "environment_busy" },
        "Couldn't start: host is serving another gateway",
      ],
      [
        { state: "refused", refusal: "agent_unavailable" },
        "Couldn't start: host can't run this agent",
      ],
      [
        { state: "refused", refusal: "environment_platform_unsupported" },
        "Couldn't start: Nessa can't install itself on this host's system",
      ],
      [
        { state: "refused", refusal: "environment_install_failed" },
        "Couldn't start: installing Nessa on the host failed",
      ],
      [{ state: "refused" }, "Couldn't start"],
      [{ state: "unreadable" }, "Not known"],
    ]
    for (const [lease, status] of cases)
      expect(conversationLeaseStatus(lease)).toBe(status)
  })
})

describe("the machine a lease names", () => {
  it("is this computer, the SSH destination, or nothing", () => {
    expect(conversationLeasePlace({ environment: "here" })).toBe("This computer")
    expect(conversationLeasePlace({ environment: "ssh", host: "me@devbox" })).toBe(
      "me@devbox",
    )
    expect(conversationLeasePlace({})).toBeUndefined()
  })
})
