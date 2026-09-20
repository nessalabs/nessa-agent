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
const oneLaunchMs =
  budgets.agent.launchMs +
  budgets.agent.startupMs +
  budgets.agent.shutdownGraceMs +
  budgets.agent.killTimeoutMs

it("waits out everything the gateway can spend, plus the stated margin", () => {
  // Two launches: a command can wait on a warm-up already paying the operating
  // system's first-execution scan, then open its own provider if that failed.
  expect(agentOperationTimeoutMs).toBe(oneLaunchMs * 2 + budgets.client.marginMs)
})

it("outlasts the gateway rather than merely differing from it", () => {
  expect(agentOperationTimeoutMs).toBeGreaterThan(oneLaunchMs * 2)
})

/** Each literal in the source is a budget the table states, not a rounding. */
it("mirrors every budget the table states", () => {
  expect(oneLaunchMs).toBe(120_000 + 45_000 + 3_000 + 2_000)
  expect(budgets.client.marginMs).toBe(10_000)
})
