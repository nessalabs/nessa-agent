import { describe, expect, it } from "vitest"

import { agentInstallRequestId } from "./agents-validate.js"

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
