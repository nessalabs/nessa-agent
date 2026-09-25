import { expect, it } from "vitest"
// Only a test reaches outside the package root; the source it checks does not.
import budgets from "../../../../protocol/defaults/agent-startup-budgets.json"
import { conversationDeleteTimeoutMs } from "./agent-budgets.js"

/**
 * The table states one delete's worst case once, and the gateway's own test
 * ties it to what the gateway spends. What matters here is that the client
 * outlasts it by the stated margin: a literal that stopped tracking the table
 * would put the client back under the gateway, drop the request, and lose the
 * typed answer.
 */
it("waits out one delete's worst case, plus the stated margin", () => {
  expect(conversationDeleteTimeoutMs).toBe(
    budgets.deletion.worstCaseMs + budgets.client.marginMs,
  )
  expect(conversationDeleteTimeoutMs).toBeGreaterThan(budgets.deletion.worstCaseMs)
})
