import { describe, expect, it, vi } from "vitest"

import { connectDevSession, SessionHealthError } from "./dev-session"
import { getSessionClient, setSessionClient } from "./handle"

describe("connectDevSession", () => {
  it("closes the client when health fails after connect", async () => {
    const close = vi.fn()
    const connect = vi.fn().mockResolvedValue({
      session: { protocol: 1 },
      server: {
        health: vi.fn().mockRejectedValue(new Error("health boom")),
      },
      close,
    })

    await expect(connectDevSession({ connect })).rejects.toBeInstanceOf(
      SessionHealthError,
    )
    expect(close).toHaveBeenCalledTimes(1)
  })

  it("returns the client when health and ping succeed", async () => {
    const close = vi.fn()
    const hello = {
      protocol: 1,
      scopes: ["server.read"],
      serverVersion: "0.1.0",
      runtimeStatus: "ready",
      policy: { maxPayloadBytes: 65536 },
      shortcuts: { version: 1, bindings: [] },
    }
    const health = { ok: true, runtimeStatus: "ready", uptimeMs: 1 }
    const ping = vi.fn().mockImplementation(async (nonce: string) => ({
      ok: true as const,
      nonce,
    }))
    const client = {
      session: hello,
      server: { health: vi.fn().mockResolvedValue(health), ping },
      close,
    }
    const connect = vi.fn().mockResolvedValue(client)

    const established = await connectDevSession({ connect })
    expect(established.client).toBe(client)
    expect(established.hello).toEqual(hello)
    expect(established.health).toEqual(health)
    expect(established.ping.ok).toBe(true)
    expect(ping).toHaveBeenCalledTimes(1)
    expect(close).not.toHaveBeenCalled()
  })

  it("closes the client when ping nonce mismatches", async () => {
    const close = vi.fn()
    const connect = vi.fn().mockResolvedValue({
      session: { protocol: 1 },
      server: {
        health: vi.fn().mockResolvedValue({
          ok: true,
          runtimeStatus: "ready",
          uptimeMs: 1,
        }),
        ping: vi.fn().mockResolvedValue({ ok: true, nonce: "wrong" }),
      },
      close,
    })

    await expect(connectDevSession({ connect })).rejects.toBeInstanceOf(
      SessionHealthError,
    )
    expect(close).toHaveBeenCalledTimes(1)
  })
})

describe("session client handle", () => {
  it("stores and clears the live client outside Redux", () => {
    const fake = { close: vi.fn() } as never
    setSessionClient(fake)
    expect(getSessionClient()).toBe(fake)
    setSessionClient(null)
    expect(getSessionClient()).toBeNull()
  })
})
