import { describe, expect, it } from "vitest"
import { LOCAL_GATEWAY_ID } from "../../model/gateway-id"
import {
  inMemoryGatewayCredentialStore,
  remoteGatewayStore,
  type GatewayStorage,
} from "./gateway-store"

function fakeStorage(): { storage: GatewayStorage; raw: () => string | null } {
  let value: string | null = null
  return {
    storage: {
      getItem: () => value,
      setItem: (_key, next) => {
        value = next
      },
    },
    raw: () => value,
  }
}

describe("remoteGatewayStore & inMemoryGatewayCredentialStore (B4, M6)", () => {
  it("persists only non-secret metadata, strips credentials, and rejects LOCAL_GATEWAY_ID", () => {
    const { storage, raw } = fakeStorage()
    const store = remoteGatewayStore(storage)

    store.write([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "Spoof",
        url: "wss://spoof.example.com",
        addedAtMs: 1,
      },
      {
        gatewayId: "gw-1",
        label: "Cloud Box",
        url: "wss://box.example.com/",
        addedAtMs: 2,
      },
    ])

    const serialized = raw() ?? ""
    expect(serialized).not.toContain("Spoof")
    expect(serialized).not.toContain("credential")
    expect(store.read()).toEqual([
      {
        gatewayId: "gw-1",
        label: "Cloud Box",
        url: "wss://box.example.com/",
        addedAtMs: 2,
      },
    ])
  })

  it("ignores corrupted entries, duplicate gatewayIds, and cleartext remote ws:// URLs on read", () => {
    const { storage } = fakeStorage()
    storage.setItem(
      "nessa.desktop.remoteGateways.v1",
      JSON.stringify([
        {
          gatewayId: LOCAL_GATEWAY_ID,
          label: "Local",
          url: "wss://a.example.com",
          addedAtMs: 1,
        },
        {
          gatewayId: "gw-bad",
          label: "Bad",
          url: "ws://remote.example.com",
          addedAtMs: 1,
        },
        { gatewayId: "gw-ok", label: "Box", url: "wss://box.example.com", addedAtMs: 2 },
        { gatewayId: "gw-ok", label: "Dup", url: "wss://dup.example.com", addedAtMs: 3 },
      ]),
    )
    const store = remoteGatewayStore(storage)
    expect(store.read()).toEqual([
      { gatewayId: "gw-ok", label: "Box", url: "wss://box.example.com/", addedAtMs: 2 },
    ])
  })

  it("stores and removes credentials strictly in memory and ignores LOCAL_GATEWAY_ID", () => {
    const creds = inMemoryGatewayCredentialStore({
      [LOCAL_GATEWAY_ID]: "nope",
      "gw-1": "tok-1",
    })
    expect(creds.read(LOCAL_GATEWAY_ID)).toBeNull()
    expect(creds.read("gw-1")).toBe("tok-1")
    creds.write("gw-2", "tok-2")
    expect(creds.read("gw-2")).toBe("tok-2")
    creds.remove("gw-2")
    expect(creds.read("gw-2")).toBeNull()
  })
})
