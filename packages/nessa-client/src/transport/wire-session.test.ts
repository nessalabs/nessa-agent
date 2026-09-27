import { describe, expect, it, vi } from "vitest"

import { NessaRpcError } from "../application/rpc-error.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"
import { WireSession } from "./wire-session.js"

describe("WireSession", () => {
  /**
   * A floor raises a shorter configured deadline; it must not lower a longer
   * one. `requestTimeoutMs` is a documented knob, and someone who raises it
   * because their agent starts slowly would otherwise be cut back to the
   * default on the very commands they raised it for.
   *
   * Controlled time, not a real wait: these assert where a deadline lands, and
   * a real one would make that a race with whatever else is running.
   */
  it("bounds the configured deadline instead of replacing it", async () => {
    vi.useFakeTimers()
    try {
      const socket = {
        readyState: 1,
        send: () => {},
        addEventListener: () => {},
        close: () => {},
      } as unknown as WebSocket

      // Raised by the floor: 25 ms configured, 10 s asked for.
      const raised = new WireSession(socket, { requestTimeoutMs: 25 })
      const raisedSettled = vi.fn()
      void raised
        .request("conversation.create", {}, { atLeastMs: 10_000 })
        .catch(raisedSettled)
      await vi.advanceTimersByTimeAsync(9_000)
      expect(raisedSettled).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(1_001)
      expect(raisedSettled).toHaveBeenCalledOnce()

      // Kept, not lowered: 10 s configured, only 25 ms asked for.
      const configured = new WireSession(socket, { requestTimeoutMs: 10_000 })
      const configuredSettled = vi.fn()
      void configured
        .request("conversation.create", {}, { atLeastMs: 25 })
        .catch(configuredSettled)
      await vi.advanceTimersByTimeAsync(9_000)
      expect(configuredSettled).not.toHaveBeenCalled()
      await vi.advanceTimersByTimeAsync(1_001)
      expect(configuredSettled).toHaveBeenCalledOnce()
    } finally {
      vi.useRealTimers()
    }
  })

  /** A cap still wins: authentication cannot usefully outlive its challenge. */
  it("caps a longer configured deadline for an operation that cannot wait", async () => {
    vi.useFakeTimers()
    try {
      const socket = {
        readyState: 1,
        send: () => {},
        addEventListener: () => {},
        close: () => {},
      } as unknown as WebSocket

      const session = new WireSession(socket, { requestTimeoutMs: 10_000 })
      const settled = vi.fn()
      void session.request("session.authenticate", {}, { atMostMs: 25 }).catch(settled)
      await vi.advanceTimersByTimeAsync(26)
      expect(settled).toHaveBeenCalledOnce()
    } finally {
      vi.useRealTimers()
    }
  })

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

  it("rejects RPC failures as NessaRpcError with wire code", async () => {
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket

    const session = new WireSession(socket, { requestTimeoutMs: 500 })
    const pending = session.request("connect", {})
    session.dispatchFrame({
      type: "res",
      id: "1",
      ok: false,
      error: { code: "unauthorized", message: "invalid auth token" },
    })

    await expect(pending).rejects.toBeInstanceOf(NessaRpcError)
    await expect(pending).rejects.toMatchObject({
      code: "unauthorized",
      message: "invalid auth token",
    })
  })

  it("rejects every pending request with the typed close code and reason", async () => {
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket
    const session = new WireSession(socket)
    const first = session.request("server.health", {})
    const second = session.request("credential.list", {})

    session.dispatchClose(4001, "unauthorized")

    for (const pending of [first, second]) {
      await expect(pending).rejects.toBeInstanceOf(NessaConnectionClosedError)
      await expect(pending).rejects.toMatchObject({ code: 4001, reason: "unauthorized" })
    }
  })
})
