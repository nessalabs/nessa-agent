import { productReadyMethods, bounds } from "../generated/product.js"
import { describe, expect, it } from "vitest"

import {
  assertHealthResult,
  assertProductSessionReady,
  parseResponseFrame,
} from "./validate.js"

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

describe("response presence schema agreement", () => {
  it("accepts only success with present payload and failure with structured error", () => {
    for (let index = 0; index < 8; index++) {
      const frame: Record<string, unknown> = {
        type: "res",
        id: "1",
        ok: Boolean(index & 4),
      }
      if (index & 2) frame.payload = null
      if (index & 1) frame.error = { code: "refused", message: "" }
      expect(parseResponseFrame(frame) !== null, `presence ${index}`).toBe(
        index === 1 || index === 6,
      )
    }
  })

  it("requires own payload evidence", () => {
    const frame = Object.assign(Object.create({ payload: null }), {
      type: "res",
      id: "1",
      ok: true,
    })
    expect(parseResponseFrame(frame)).toBeNull()
  })
})

it("accepts the actual manifest ready inventory and refuses one beyond its published bound", () => {
  const ready = {
    version: 1,
    gatewayId: "gateway",
    principalId: "owner",
    organizationId: "org",
    membershipId: "member",
    credentialId: "credential",
    audienceId: "gateway",
    expiresAt: null,
    grants: [],
    methods: [...productReadyMethods],
  }
  expect(assertProductSessionReady(ready)).toBe(ready)
  expect(ready.methods.length).toBe(bounds.maxReadyMethods)
  expect(() =>
    assertProductSessionReady({ ...ready, methods: [...ready.methods, "extra"] }),
  ).toThrow("invalid methods")
})
