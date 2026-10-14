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

describe("the agent catalog's places", () => {
  // At most 16 SSH destinations and 8 connected paired environments.
  const places = (count: number) => [
    ...Array.from({ length: Math.min(count, 16) }, (_, index) => `host${index}`),
    ...Array.from(
      { length: Math.max(count - 16, 0) },
      (_, index) => `peer:${index.toString(16).padStart(2, "0").repeat(32)}`,
    ),
  ]

  it("accepts the SSH hosts and connected paired environments together", () => {
    expect(
      agentsList({
        agents: [],
        environments: places(24),
        commandHosts: places(24),
        nativeHooks: { claude: "off", codex: "off" },
      }).environments,
    ).toHaveLength(24)
  })

  it("refuses more places than both kinds can name", () => {
    expect(() =>
      agentsList({
        agents: [],
        environments: places(25),
        commandHosts: [],
        nativeHooks: { claude: "off", codex: "off" },
      }),
    ).toThrow("Invalid agent catalog environments")
  })
})
