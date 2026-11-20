import { NessaRpcError } from "@nessa/client"
import { describe, expect, it } from "vitest"
import { decisionsGateway, type DecisionsClient } from "./decisions-gateway"

function source(decisions: DecisionsClient["conversation"]["decisions"]) {
  return decisionsGateway({
    connected: async () => ({
      conversation: { decisions, pauseAutoPick: async () => false },
    }),
  })
}

describe("decisionsGateway", () => {
  it("reads a conversation the gateway does not hold as no decisions", async () => {
    const reads = source(async () => {
      throw new NessaRpcError("conversation_not_found", "No such conversation")
    })
    await expect(reads.decisions("not-started")).resolves.toEqual([])
  })

  it("still fails a read the gateway refused for another reason", async () => {
    const reads = source(async () => {
      throw new NessaRpcError("unavailable", "Try again")
    })
    await expect(reads.decisions("chat")).rejects.toMatchObject({ code: "unavailable" })
  })
})
