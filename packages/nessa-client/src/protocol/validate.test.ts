import { describe, expect, it } from "vitest"

import {
  assertHelloOk,
  assertHealthResult,
  assertPingResult,
  parseResponseFrame,
} from "./validate.js"

describe("protocol validate", () => {
  it("rejects hello payloads with invalid scopes or policy", () => {
    expect(() =>
      assertHelloOk({
        protocol: 1,
        scopes: [{}],
        serverVersion: "1.0.0",
        runtimeStatus: "ready",
        policy: { maxPayloadBytes: 65536 },
        shortcuts: { version: 1, bindings: [] },
      }),
    ).toThrow("invalid scopes")

    expect(() =>
      assertHelloOk({
        protocol: 1,
        scopes: ["server.read"],
        serverVersion: "1.0.0",
        runtimeStatus: "ready",
        policy: { maxPayloadBytes: -1 },
        shortcuts: { version: 1, bindings: [] },
      }),
    ).toThrow("invalid policy.maxPayloadBytes")
  })

  it("rejects hello payloads with invalid shortcuts", () => {
    expect(() =>
      assertHelloOk({
        protocol: 1,
        scopes: ["server.read"],
        serverVersion: "1.0.0",
        runtimeStatus: "ready",
        policy: { maxPayloadBytes: 65536 },
        shortcuts: { version: 2, bindings: [] },
      }),
    ).toThrow("unsupported shortcuts.version")
  })

  it("rejects health payloads with invalid runtime status", () => {
    expect(() =>
      assertHealthResult({
        ok: true,
        runtimeStatus: "garbage",
        uptimeMs: 1,
      }),
    ).toThrow("invalid runtimeStatus")
  })

  it("rejects ping payloads without an echoed nonce", () => {
    expect(() => assertPingResult({ ok: true })).toThrow("missing nonce")
    expect(() => assertPingResult({ ok: false, nonce: "x" })).toThrow(
      "not a valid PingResult",
    )
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
