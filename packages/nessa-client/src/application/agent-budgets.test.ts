import { expect, it } from "vitest"
// Only a test reaches outside the package root; the source it checks does not.
import budgets from "../../../../protocol/defaults/agent-startup-budgets.json"
import { agentOperationTimeoutMs } from "./agent-budgets.js"

/**
 * The table is what the gateway compiles into its own budgets, so this is the
 * relationship that matters: the client outlasts the gateway's worst case. A
 * literal here that stopped tracking the table would put the client back under
 * the gateway, delete the request, and lose the typed answer — the defect the
 * table exists to prevent.
 */
it("waits out everything the gateway can spend, plus the stated margin", () => {
  expect(agentOperationTimeoutMs).toBe(
    budgets.agent.startupMs +
      budgets.agent.shutdownGraceMs +
      budgets.agent.killTimeoutMs +
      budgets.client.marginMs,
  )
})

it("outlasts the gateway rather than merely differing from it", () => {
  const gatewayWorstCaseMs =
    budgets.agent.startupMs + budgets.agent.shutdownGraceMs + budgets.agent.killTimeoutMs
  expect(agentOperationTimeoutMs).toBeGreaterThan(gatewayWorstCaseMs)
})
