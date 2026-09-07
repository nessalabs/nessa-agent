import { describe, expect, it, vi } from "vitest"
import {
  ManagedSession,
  NessaSessionUnavailableError,
  type ConnectedSession,
} from "./managed-session.js"
import { NessaClientConfig } from "./client-config.js"
import { NessaConnectionClosedError } from "./connection-closed-error.js"
import { NessaRpcError } from "./rpc-error.js"
import { RetryableConnectError } from "./connect-retry.js"
import type { SessionTransport } from "./session-port.js"
import type { ProductSessionReady } from "../protocol/product-types.js"

class Transport implements SessionTransport {
  termination: NessaConnectionClosedError | undefined
  listeners = new Set<(error: NessaConnectionClosedError) => void>()
  events = new Map<string, Set<(value: unknown) => void>>()
  request = vi.fn(async () => "ok")
  onClose(handler: (error: NessaConnectionClosedError) => void) {
    this.listeners.add(handler)
    return () => {
      this.listeners.delete(handler)
    }
  }
  onEvent(event: string, handler: (value: unknown) => void) {
    const set = this.events.get(event) ?? new Set()
    set.add(handler)
    this.events.set(event, set)
    return () => {
      set.delete(handler)
    }
  }
  drop(code = 1006, reason = "") {
    if (this.termination) return
    this.termination = new NessaConnectionClosedError(code, reason)
    for (const listener of [...this.listeners]) listener(this.termination)
    this.listeners.clear()
    this.events.clear()
  }
  close() {
    this.drop(1000)
  }
}
const connected = (wire = new Transport()): ConnectedSession & { wire: Transport } => ({
  wire,
  ready: { sessionId: "fixture" } as unknown as ProductSessionReady,
  profile: "product",
})
const flush = async () => {
  for (let i = 0; i < 12; i++) await Promise.resolve()
}
const config = new NessaClientConfig({ reconnect: { jitter: false } })
const timing = () => ({
  wait: vi.fn(async (_ms: number, _signal: AbortSignal) => {}),
  random: () => 0,
})

describe("persistent session", () => {
  it("preserves facade/subscriptions while refusing calls during recovery", async () => {
    const first = connected(),
      next = connected(),
      clock = timing()
    const connect = vi.fn(async () => next)
    const client = new ManagedSession(first, config, connect, clock)
    const event = vi.fn(),
      close = vi.fn()
    client.onEvent("sample", event)
    client.onClose(close)
    first.wire.drop()
    expect(client.state.status).toBe("reconnecting")
    await expect(client.request("command", {})).rejects.toBeInstanceOf(
      NessaSessionUnavailableError,
    )
    await flush()
    next.wire.events.get("sample")?.forEach((handler) => handler(123))
    expect(event).toHaveBeenCalledWith(123)
    expect(await client.request("health", {})).toBe("ok")
    expect(first.wire.request).not.toHaveBeenCalled()
    expect(close).not.toHaveBeenCalled()
    client.close()
    expect(close).toHaveBeenCalledTimes(1)
    expect(next.wire.termination?.code).toBe(1000)
  })

  it.each([1000, 1008, 4001, 4002, 4003, 4004, 4005, 4999])(
    "never reconnects terminal close %i",
    async (code) => {
      const first = connected(),
        connect = vi.fn()
      const client = new ManagedSession(first, config, connect, timing())
      first.wire.drop(code, '{"retryable":true}')
      await flush()
      expect(connect).not.toHaveBeenCalled()
      expect(client.state.status).toBe("closed")
    },
  )

  it.each([1001, 1006, 1011, 1012, 1013, 4006, 4010])(
    "reconnects transient close %i",
    async (code) => {
      const first = connected(),
        next = connected(),
        connect = vi.fn(async () => next)
      const client = new ManagedSession(first, config, connect, timing())
      first.wire.drop(code)
      await flush()
      expect(connect).toHaveBeenCalledTimes(1)
      expect(client.state.status).toBe("connected")
      client.close()
    },
  )

  it("exhausts the configured budget with bounded backoff and one final notification", async () => {
    const first = connected(),
      clock = timing(),
      connect = vi.fn(async () => {
        throw new RetryableConnectError("offline")
      })
    const client = new ManagedSession(first, config, connect, clock),
      close = vi.fn()
    client.onClose(() => {
      throw Error("consumer")
    })
    client.onClose(close)
    first.wire.drop()
    await flush()
    expect(connect).toHaveBeenCalledTimes(3)
    expect(clock.wait.mock.calls.map((call) => call[0])).toEqual([250, 500, 1000])
    expect(close).toHaveBeenCalledTimes(1)
    client.close()
    expect(close).toHaveBeenCalledTimes(1)
  })

  it("stops immediately when fresh authentication is rejected", async () => {
    const first = connected(),
      connect = vi.fn(async () => {
        throw new NessaRpcError("unauthorized", "denied")
      })
    const client = new ManagedSession(first, config, connect, timing())
    first.wire.drop()
    await flush()
    expect(connect).toHaveBeenCalledTimes(1)
    expect(client.state.status).toBe("closed")
  })

  it("can disable established reconnection independently", async () => {
    const first = connected(),
      connect = vi.fn()
    const client = new ManagedSession(
      first,
      new NessaClientConfig({ reconnect: { enabled: false } }),
      connect,
      timing(),
    )
    first.wire.drop()
    await flush()
    expect(client.state.status).toBe("closed")
    expect(connect).not.toHaveBeenCalled()
  })

  it("cancels a pending backoff and never resurrects after explicit close", async () => {
    const first = connected(),
      connect = vi.fn(),
      clock = {
        random: () => 0,
        wait: vi.fn(
          (_ms: number, signal: AbortSignal) =>
            new Promise<void>((_resolve, reject) =>
              signal.addEventListener("abort", () => reject(signal.reason), {
                once: true,
              }),
            ),
        ),
      }
    const client = new ManagedSession(first, config, connect, clock)
    first.wire.drop()
    client.close()
    await flush()
    expect(connect).not.toHaveBeenCalled()
    expect(client.state.status).toBe("closed")
  })

  it("closes late successful attempts when the caller closes during authentication", async () => {
    const first = connected(),
      next = connected()
    let resolve!: (session: ConnectedSession) => void
    const connect = vi.fn(
      () =>
        new Promise<ConnectedSession>((done) => {
          resolve = done
        }),
    )
    const client = new ManagedSession(first, config, connect, timing())
    first.wire.drop()
    await flush()
    client.close()
    resolve(next)
    await flush()
    expect(next.wire.termination).toBeDefined()
    expect(client.state.status).toBe("closed")
  })

  it("honors bounded retryAfter only for matching transient codes", async () => {
    const first = connected(),
      clock = timing(),
      next = connected()
    const client = new ManagedSession(first, config, async () => next, clock)
    first.wire.drop(
      1013,
      JSON.stringify({
        code: "gateway_overloaded",
        retryable: true,
        retryAfterMs: 999999,
      }),
    )
    await flush()
    expect(clock.wait).toHaveBeenCalledWith(60000, expect.any(AbortSignal))
    client.close()
    expect(
      new NessaConnectionClosedError(
        4002,
        '{"code":"credential_revoked","retryable":true}',
      ).retryable,
    ).toBe(false)
  })

  it("survives 500 independently substituted sessions with repeated interruptions", async () => {
    const clients = Array.from({ length: 500 }, () => {
      let active = connected()
      const client = new ManagedSession(
        active,
        config,
        async () => {
          active = connected()
          return active
        },
        timing(),
      )
      return { client, drop: () => active.wire.drop(), active: () => active }
    })
    for (let round = 0; round < 5; round++) {
      clients.forEach((item) => item.drop())
      await flush()
      expect(clients.every((item) => item.client.state.status === "connected")).toBe(true)
    }
    clients.forEach((item) => item.client.close())
    expect(
      clients.every(
        (item) =>
          item.active().wire.listeners.size === 0 && item.active().wire.events.size === 0,
      ),
    ).toBe(true)
  })
})
