import { describe, expect, it } from "vitest"
import {
  LOCAL_GATEWAY_ID,
  gatewaySectionId,
  gatewayStatusLabel,
  isTerminalId,
  normalizeGatewayUrl,
  parseQualifiedId,
  parseRemoteQualifiedId,
  qualifyEntityId,
  removeRemoteGateway,
  upsertRemoteGateway,
  type PersistedRemoteGateway,
} from "./gateway-id"

describe("gateway-id composite identity (G10)", () => {
  it("passes local gateway IDs through unchanged and parses them back as local", () => {
    expect(qualifyEntityId(LOCAL_GATEWAY_ID, "default")).toBe("default")
    expect(parseQualifiedId("default")).toEqual({
      gatewayId: LOCAL_GATEWAY_ID,
      rawId: "default",
    })
    expect(parseRemoteQualifiedId("default")).toBeNull()
    expect(gatewaySectionId(LOCAL_GATEWAY_ID)).toBe("gateway")
  })

  it("qualifies remote IDs canonically and keeps identical raw IDs from two gateways distinct", () => {
    const raw = "default"
    const a = qualifyEntityId("gw-alpha", raw)
    const b = qualifyEntityId("gw-beta", raw)
    expect(a).not.toBe(b)
    expect(a).not.toBe(raw)
    expect(parseQualifiedId(a)).toEqual({ gatewayId: "gw-alpha", rawId: raw })
    expect(parseQualifiedId(b)).toEqual({ gatewayId: "gw-beta", rawId: raw })
  })

  it("handles colons, percent signs, and special characters in gatewayId and rawId", () => {
    const gwId = "org:box%1/prod"
    const rawId = "chan:foo%bar"
    const encoded = qualifyEntityId(gwId, rawId)
    expect(parseQualifiedId(encoded)).toEqual({ gatewayId: gwId, rawId })
  })

  it("rejects malformed or non-canonical remote prefixes", () => {
    expect(parseRemoteQualifiedId("gw:")).toBeNull()
    expect(parseRemoteQualifiedId("gw:onlyone")).toBeNull()
    expect(parseRemoteQualifiedId("gw:a:b:c")).toBeNull()
    expect(parseRemoteQualifiedId("gw::raw")).toBeNull()
    expect(parseRemoteQualifiedId(`gw:${LOCAL_GATEWAY_ID}:raw`)).toBeNull()
    expect(() => qualifyEntityId("", "raw")).toThrow()
  })

  it("validates local and remote qualified terminal IDs for pane keys", () => {
    const rawTerm = "t_0123456789abcdef0123456789abcdef"
    expect(isTerminalId(rawTerm)).toBe(true)
    expect(isTerminalId(qualifyEntityId("cloud-box", rawTerm))).toBe(true)
    expect(isTerminalId("not-a-terminal")).toBe(false)
    expect(isTerminalId(qualifyEntityId("cloud-box", "not-a-terminal"))).toBe(false)
  })
})

describe("remote gateway record helpers (G7, G11)", () => {
  const boxA: PersistedRemoteGateway = {
    gatewayId: "gw-1",
    label: "Cloud Box",
    url: "wss://box1.example.com",
    addedAtMs: 100,
  }

  it("deduplicates by stable gatewayId on re-pair (G7)", () => {
    const first = upsertRemoteGateway([], boxA)
    expect(first).toHaveLength(1)
    const updated = upsertRemoteGateway(first, {
      ...boxA,
      label: "Cloud Box Renamed",
      url: "wss://box1-new.example.com",
    })
    expect(updated).toHaveLength(1)
    expect(updated[0].url).toBe("wss://box1-new.example.com")
    expect(updated[0].label).toBe("Cloud Box Renamed")
  })

  it("removes a gateway cleanly by gatewayId (G11)", () => {
    const list = upsertRemoteGateway([boxA], {
      ...boxA,
      gatewayId: "gw-2",
      label: "Devbox",
    })
    expect(removeRemoteGateway(list, "gw-1").map((g) => g.gatewayId)).toEqual(["gw-2"])
  })

  it("allows wss:// for remote hosts, ws:// only for numeric loopback, and rejects userinfo (M5)", () => {
    expect(normalizeGatewayUrl("wss://gw.example.com")).toBe("wss://gw.example.com/")
    expect(normalizeGatewayUrl("ws://127.0.0.1:18789")).toBe("ws://127.0.0.1:18789/")
    expect(normalizeGatewayUrl("ws://[::1]:18789")).toBe("ws://[::1]:18789/")
    expect(normalizeGatewayUrl("ws://localhost:18789")).toBeNull()
    expect(normalizeGatewayUrl("wss://user:pass@gw.example.com")).toBeNull()
    expect(normalizeGatewayUrl("ws://evil.example.com:18789")).toBeNull()
    expect(normalizeGatewayUrl("http://127.0.0.1:18789")).toBeNull()
    expect(normalizeGatewayUrl("not a url")).toBeNull()
  })

  it("provides plain-language labels for every connection state", () => {
    expect(gatewayStatusLabel("connected")).toBe("Connected")
    expect(gatewayStatusLabel("auth-refused")).toBe("Re-pair required")
    expect(gatewayStatusLabel("waking")).toBe("Waking")
  })
})
