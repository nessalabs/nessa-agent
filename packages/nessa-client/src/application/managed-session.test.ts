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
  closed = false
  close() {
    this.closed = true
    this.drop(1000)
  }
}
const connected = (wire = new Transport()): ConnectedSession & { wire: Transport } => ({
  wire,
  ready: { sessionId: "fixture" } as unknown as ProductSessionReady,
  profile: "product",
  url: "ws://127.0.0.1:7421/session",
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

  it("does not publish stale reconnecting state after an observer closes reentrantly", async () => {
    const first = connected()
    const connect = vi.fn()
    const client = new ManagedSession(first, config, connect, timing())
    const seen: string[] = []
    client.onState((state) => {
      seen.push(`first:${state.status}`)
      if (state.status === "reconnecting") client.close()
    })
    client.onState((state) => seen.push(`second:${state.status}`))

    first.wire.drop()
    await flush()

    expect(seen).toEqual(["first:reconnecting", "first:closed", "second:closed"])
    expect(client.state.status).toBe("closed")
    expect(connect).not.toHaveBeenCalled()
  })

  it("closes a replacement wire without publishing stale connected state after reentrant close", async () => {
    const first = connected()
    const next = connected()
    const client = new ManagedSession(first, config, async () => next, timing())
    const seen: string[] = []
    client.onState((state) => {
      seen.push(`first:${state.status}`)
      if (state.status === "connected") client.close()
    })
    client.onState((state) => seen.push(`second:${state.status}`))

    first.wire.drop()
    await flush()

    expect(seen).toEqual([
      "first:reconnecting",
      "second:reconnecting",
      "first:connected",
      "first:closed",
      "second:closed",
    ])
    expect(client.state.status).toBe("closed")
    expect(next.wire.termination?.code).toBe(1000)
  })

  it("continues nonterminal publication after a throwing observer", async () => {
    const first = connected()
    const next = connected()
    const client = new ManagedSession(first, config, async () => next, timing())
    const seen = vi.fn()
    client.onState(() => {
      throw new Error("observer")
    })
    client.onState(seen)

    first.wire.drop()
    await flush()

    expect(seen.mock.calls.map(([state]) => state.status)).toEqual([
      "reconnecting",
      "connected",
    ])
    expect(client.state.status).toBe("connected")
    client.close()
  })

  it("finishes the closed notification generation when observers close reentrantly", () => {
    const first = connected()
    const client = new ManagedSession(first, config, vi.fn(), timing())
    const seen: string[] = []
    const closed = vi.fn(() => client.close())
    client.onState((state) => {
      seen.push(`first:${state.status}`)
      if (state.status === "closed") client.close()
    })
    client.onState((state) => seen.push(`second:${state.status}`))
    client.onClose(closed)

    client.close()

    expect(seen).toEqual(["first:closed", "second:closed"])
    expect(closed).toHaveBeenCalledOnce()
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

  it("spends the recovery budget on replacements that arrive already closed", async () => {
    const bounded = new NessaClientConfig({
      reconnect: {
        jitter: false,
        maxAttempts: 3,
        initialDelayMs: 250,
        maxDelayMs: 2_000,
      },
    })
    const first = connected(),
      clock = timing()
    const replacements: ReturnType<typeof connected>[] = []
    const connect = vi.fn(async () => {
      const next = connected()
      // Closed between the handshake and adoption: never a connected session.
      next.wire.drop(1006)
      replacements.push(next)
      return next
    })
    const client = new ManagedSession(first, bounded, connect, clock)
    const attempts: number[] = []
    const closed = vi.fn()
    client.onState((state) => {
      if (state.status === "reconnecting") attempts.push(state.attempt)
    })
    client.onClose(closed)
    // A live subscription, so the handoff onto each dead replacement and back
    // off it again is actually exercised rather than assumed.
    const event = vi.fn()
    client.onEvent("sample", event)

    first.wire.drop()
    await flush()

    expect(connect).toHaveBeenCalledTimes(3)
    expect(attempts).toEqual([1, 2, 3])
    expect(clock.wait.mock.calls.map(([ms]) => ms)).toEqual([250, 500, 1000])
    expect(client.state.status).toBe("closed")
    expect(closed).toHaveBeenCalledTimes(1)
    // Each replacement took the subscription and gave it back. Counting handlers
    // rather than event names: the name stays registered with an empty set.
    expect(replacements).toHaveLength(3)
    const handlers = replacements.flatMap((next) =>
      [...next.wire.events.values()].map((listeners) => listeners.size),
    )
    expect(handlers).not.toHaveLength(0)
    expect(handlers.every((count) => count === 0)).toBe(true)
    // Released, so an emission on a refused replacement reaches nobody.
    for (const next of replacements) {
      for (const listeners of next.wire.events.values()) {
        for (const handler of listeners) handler("late")
      }
    }
    expect(event).not.toHaveBeenCalled()
    // And each refused transport was let go rather than left open. Checking the
    // close, not the termination: the stub sets that before adoption ever sees it.
    expect(replacements.every((next) => next.wire.closed)).toBe(true)
  })

  // A control rather than a regression: this held before the budget fix too.
  // It is here so the fix cannot buy a bounded budget by making every closed
  // replacement retryable.
  it("ends recovery when a replacement arrives closed for a terminal reason", async () => {
    const first = connected(),
      clock = timing()
    const connect = vi.fn(async () => {
      const next = connected()
      next.wire.drop(4002, '{"code":"credential_revoked","retryable":false}')
      return next
    })
    const client = new ManagedSession(first, config, connect, clock)
    const closed = vi.fn()
    const attempts: number[] = []
    client.onState((state) => {
      if (state.status === "reconnecting") attempts.push(state.attempt)
    })
    client.onClose(closed)

    first.wire.drop()
    await flush()

    // One attempt spent, then stopped: a terminal replacement ends recovery
    // rather than using up the remaining budget.
    expect(connect).toHaveBeenCalledTimes(1)
    expect(attempts).toEqual([1])
    expect(client.state.status).toBe("closed")
    expect(closed).toHaveBeenCalledTimes(1)
  })

  it("keeps one budget when a transport reports its close on registration", async () => {
    // `SessionTransport` does not say a close handler cannot run during
    // registration, and one that does would otherwise start recovery from
    // inside adoption — a second loop with a budget of its own.
    class Replaying extends Transport {
      onClose(handler: (error: NessaConnectionClosedError) => void) {
        const off = super.onClose(handler)
        if (this.termination) handler(this.termination)
        return off
      }
    }
    const bounded = new NessaClientConfig({
      reconnect: {
        jitter: false,
        maxAttempts: 2,
        initialDelayMs: 250,
        maxDelayMs: 2_000,
      },
    })
    const first = connected(),
      clock = timing()
    const replacements: ReturnType<typeof connected>[] = []
    const connect = vi.fn(async () => {
      const next = connected(new Replaying())
      next.wire.drop(1006)
      replacements.push(next)
      return next
    })
    const client = new ManagedSession(first, bounded, connect, clock)
    const attempts: number[] = []
    const closed = vi.fn()
    client.onState((state) => {
      if (state.status === "reconnecting") attempts.push(state.attempt)
    })
    client.onClose(closed)
    // A live subscription, so the handoff onto each replacement and back off it
    // again is exercised rather than assumed.
    const event = vi.fn()
    client.onEvent("sample", event)

    first.wire.drop()
    await flush()

    expect(connect).toHaveBeenCalledTimes(2)
    expect(attempts).toEqual([1, 2])
    expect(clock.wait.mock.calls.map(([ms]) => ms)).toEqual([250, 500])
    expect(closed).toHaveBeenCalledTimes(1)
    expect(client.state.status).toBe("closed")
    // No replacement kept an event subscription. Counting handlers, and
    // refusing an empty set, because an unsubscribed name stays in the map.
    expect(replacements).toHaveLength(2)
    const handlers = replacements.flatMap((next) =>
      [...next.wire.events.values()].map((listeners) => listeners.size),
    )
    expect(handlers).not.toHaveLength(0)
    expect(handlers.every((count) => count === 0)).toBe(true)
    expect(event).not.toHaveBeenCalled()
    // Adoption took each transport and gave it back: a refused replacement is
    // closed rather than left open, the way `finish` closes the live one.
    expect(replacements.every((next) => next.wire.closed)).toBe(true)
  })

  it("adopts a live replacement after one that arrived already closed", async () => {
    const first = connected(),
      live = connected()
    const connect = vi.fn(async () => {
      if (connect.mock.calls.length === 1) {
        const stale = connected()
        stale.wire.drop(1006)
        return stale
      }
      return live
    })
    const clock = timing()
    const client = new ManagedSession(first, config, connect, clock)
    const event = vi.fn()
    const attempts: number[] = []
    client.onState((state) => {
      if (state.status === "reconnecting") attempts.push(state.attempt)
    })
    client.onEvent("sample", event)

    first.wire.drop()
    await flush()

    expect(connect).toHaveBeenCalledTimes(2)
    // The second attempt is the second attempt. A nested recovery would report
    // [1, 1] here and back off from the start again, which is the whole defect.
    expect(attempts).toEqual([1, 2])
    expect(clock.wait.mock.calls.map(([ms]) => ms)).toEqual([250, 500])
    expect(client.state.status).toBe("connected")
    live.wire.events.get("sample")?.forEach((handler) => handler(7))
    expect(event).toHaveBeenCalledWith(7)
    client.close()
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
