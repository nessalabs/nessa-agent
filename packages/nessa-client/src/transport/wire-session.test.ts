import { describe, expect, it, vi } from "vitest"

import { NessaRequestTooLargeError } from "../application/request-too-large-error.js"
import { NessaRpcError } from "../application/rpc-error.js"
import { NessaConnectionClosedError } from "../application/connection-closed-error.js"
import { WireSession } from "./wire-session.js"
import { bounds, ProductMethod } from "../generated/product.js"

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

  it("refuses, before sending, a request whose frame is past the gateway's message limit", async () => {
    const sent: string[] = []
    const socket = {
      readyState: 1,
      send: (text: string) => void sent.push(text),
      addEventListener: () => {},
      close: () => {},
    } as unknown as WebSocket
    const session = new WireSession(socket, { requestTimeoutMs: 500 })
    // Each quote is escaped once more in the frame: 2 bytes become 4.
    const quotes = '"'.repeat(bounds.maxRequestFrameBytes / 2)
    const refused = session.request("conversation.send", { text: quotes })
    await expect(refused).rejects.toBeInstanceOf(NessaRequestTooLargeError)
    await expect(refused).rejects.toMatchObject({ method: "conversation.send" })
    expect(sent).toEqual([])
    // At the limit exactly, it is sent.
    const envelope = JSON.stringify({
      type: "req",
      id: "2",
      method: "m",
      params: { t: "" },
    })
    void session
      .request("m", { t: "x".repeat(bounds.maxRequestFrameBytes - envelope.length) })
      .catch(() => {})
    expect(sent).toHaveLength(1)
    expect(new TextEncoder().encode(sent[0]).byteLength).toBe(bounds.maxRequestFrameBytes)
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

  it("accepts a larger response only for a pending record read", async () => {
    const listeners = new Map<string, (event: { data: string }) => void>()
    const close = vi.fn()
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: (kind: string, handler: (event: { data: string }) => void) => {
        listeners.set(kind, handler)
      },
      close,
    } as unknown as WebSocket
    const session = new WireSession(socket)
    const payload = { content: "x".repeat(bounds.maxOrdinaryResponseBytes) }
    const wire = (id: string) => JSON.stringify({ type: "res", id, ok: true, payload })
    expect(new TextEncoder().encode(wire("1")).length).toBeLessThan(
      bounds.maxRecordResponseBytes,
    )

    const record = session.request(ProductMethod.ConversationRecordsPage, {})
    listeners.get("message")?.({ data: wire("1") })
    await expect(record).resolves.toEqual(payload)

    const ordinary = session.request(ProductMethod.ServerHealth, {})
    listeners.get("message")?.({ data: wire("2") })
    await expect(ordinary).rejects.toMatchObject({ code: 1009 })
    expect(close).toHaveBeenCalledOnce()
  })

  it("closes above the record ceiling before parsing JSON", async () => {
    const listeners = new Map<string, (event: { data: string }) => void>()
    const close = vi.fn()
    const socket = {
      readyState: 1,
      send: () => {},
      addEventListener: (kind: string, handler: (event: { data: string }) => void) => {
        listeners.set(kind, handler)
      },
      close,
    } as unknown as WebSocket
    const session = new WireSession(socket)
    const pending = session.request(ProductMethod.ConversationRecordsPage, {})
    listeners.get("message")?.({
      data: "😀".repeat(Math.floor(bounds.maxRecordResponseBytes / 4) + 1),
    })
    await expect(pending).rejects.toMatchObject({ code: 1009 })
    expect(close).toHaveBeenCalledOnce()
  })
})
