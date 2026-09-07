import { describe, expect, it } from "vitest"
import { NessaClient } from "../presentation/nessa-client.js"
import { isLoopbackWebSocketUrl, resolveConnectOptions } from "./resolve-options.js"
import { StageConfigError } from "./stage.js"
const base = {
  role: "surface" as const,
  surface: { kind: "panel" as const, instance: "test" },
  client: { id: "test", version: "0.1.0", platform: "node" as const },
  auth: { credential: "assigned-credential" },
}
describe("resolveConnectOptions", () => {
  it("defaults dev URL to the authenticated session endpoint", () => {
    const result = resolveConnectOptions(base, NessaClient.defaultUrl)
    expect(result.stage).toBe("dev")
    expect(result.url).toBe(`${NessaClient.defaultUrl}/session`)
    expect(result.auth.credential).toBe("assigned-credential")
  })
  it("requires a loaded credential in every stage", () => {
    for (const stage of ["dev", "ci", "prod", "alpha"] as const) {
      expect(() =>
        resolveConnectOptions(
          { ...base, stage, url: NessaClient.defaultUrl, auth: undefined },
          NessaClient.defaultUrl,
        ),
      ).toThrow(/auth\.credential/)
    }
  })
  it("requires an explicit URL outside dev", () => {
    expect(() =>
      resolveConnectOptions({ ...base, stage: "ci" }, NessaClient.defaultUrl),
    ).toThrow(StageConfigError)
  })
  it("requires wss for remote production endpoints", () => {
    expect(() =>
      resolveConnectOptions(
        { ...base, stage: "prod", url: "ws://example.com" },
        NessaClient.defaultUrl,
      ),
    ).toThrow(/wss:/)
    expect(
      resolveConnectOptions(
        { ...base, stage: "prod", url: "wss://example.com/session" },
        NessaClient.defaultUrl,
      ).url,
    ).toBe("wss://example.com/session")
  })
  it("requires TLS for remote endpoints in every stage", () => {
    for (const stage of ["dev", "ci", "prod", "alpha"] as const) {
      expect(() =>
        resolveConnectOptions(
          { ...base, stage, url: "ws://example.com/session" },
          NessaClient.defaultUrl,
        ),
      ).toThrow(/wss:/)
      expect(
        resolveConnectOptions(
          { ...base, stage, url: "wss://example.com/session" },
          NessaClient.defaultUrl,
        ).url,
      ).toBe("wss://example.com/session")
    }
  })
  it("trusts numeric loopback addresses and rejects URL user information", () => {
    expect(isLoopbackWebSocketUrl("ws://127.0.0.1:7420")).toBe(true)
    expect(isLoopbackWebSocketUrl("ws://[::1]:7420")).toBe(true)
    expect(isLoopbackWebSocketUrl("wss://localhost:7420")).toBe(false)
    for (const url of [
      "ws://127.0.0.1:7420@evil.example/session",
      "wss://user@127.0.0.1",
    ]) {
      expect(() =>
        resolveConnectOptions({ ...base, url }, NessaClient.defaultUrl),
      ).toThrow(/user information/)
    }
  })
  it("rejects empty and oversized credentials", () => {
    for (const credential of ["", "a".repeat(16385)]) {
      expect(() =>
        resolveConnectOptions({ ...base, auth: { credential } }, NessaClient.defaultUrl),
      ).toThrow(/auth\.credential/)
    }
  })
})
