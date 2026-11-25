import { describe, expect, it } from "vitest"

import { agentInstallRequestId, agentsList } from "./agents-validate.js"

describe("agent installation request ID", () => {
  it.each(["\uD800", "\uDC00", "valid\uD800", "\uDC00valid"])(
    "rejects an unpaired UTF-16 surrogate before encoding",
    (requestId) => {
      expect(() => agentInstallRequestId(requestId)).toThrow(
        "Invalid agent installation request ID",
      )
    },
  )

  it("preserves a valid surrogate pair", () => {
    expect(agentInstallRequestId("\uD83D\uDCA9")).toBe("\uD83D\uDCA9")
  })
})

describe("the agent catalog", () => {
  it("names its agents and hooks switches, and no other place to run", () => {
    const catalog = { agents: [], nativeHooks: { claude: "off", codex: "off" } }
    expect(agentsList(catalog)).toEqual(catalog)
    // Every gateway runs only its own agents (ADR 1417): an older gateway's
    // places are not a catalog this client reads.
    expect(() => agentsList({ ...catalog, environments: [], commandHosts: [] })).toThrow()
  })
})
