import { describe, expect, it } from "vitest"

import { WireSession } from "./wire-session.js"

describe("WireSession", () => {
  it("rejects requests that never receive a correlated response", async () => {
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket

    const session = new WireSession(socket, { requestTimeoutMs: 25 })
    await expect(session.request("server.health", {})).rejects.toThrow(
      "request timeout: server.health",
    )
  })

  it("ignores malformed response frames without resolving pending requests", async () => {
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket

    const session = new WireSession(socket, { requestTimeoutMs: 25 })
    const pending = session.request("connect", {})
    session.dispatchFrame({
      type: "res",
      id: "",
      ok: true,
      payload: {},
    } as never)

    await expect(pending).rejects.toThrow("request timeout: connect")
  })

  it("cleans up pending state when send throws", async () => {
    const socket = {
      readyState: 1,
      send: () => {
        throw new Error("send failed")
      },
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket

    const session = new WireSession(socket, { requestTimeoutMs: 500 })
    await expect(session.request("connect", {})).rejects.toThrow("send failed")
    await expect(session.request("connect", {})).rejects.toThrow("send failed")
  })
})
