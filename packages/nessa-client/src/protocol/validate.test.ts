import { describe, expect, it } from "vitest"

import { assertHealthResult, parseResponseFrame } from "./validate.js"

describe("protocol validate", () => {
  it("rejects health payloads with invalid runtime status", () => {
    expect(() =>
      assertHealthResult({
        ok: true,
        runtimeStatus: "garbage",
        uptimeMs: 1,
      }),
    ).toThrow("invalid runtimeStatus")
  })

  it("rejects contradictory response envelopes", () => {
    expect(
      parseResponseFrame({
        type: "res",
        id: "1",
        ok: true,
        payload: {},
        error: { code: "x", message: "y" },
      }),
    ).toBeNull()

    expect(
      parseResponseFrame({
        type: "res",
        id: "1",
        ok: false,
        payload: {},
        error: { code: "x", message: "y" },
      }),
    ).toBeNull()
  })
})
