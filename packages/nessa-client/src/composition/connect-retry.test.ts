import { NessaClientConfig } from "../application/client-config.js"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { NessaClient } from "../presentation/nessa-client.js"
import { NessaProtocolCompatibilityError } from "../application/protocol-compatibility-error.js"
import {
  retryProductConnection,
  RetryableConnectError,
  resolveConnectRetry,
} from "../application/connect-retry.js"

type Scenario =
  | "ready"
  | "no-open"
  | "no-challenge"
  | "drop"
  | "timeout"
  | "denied"
  | "auth-close"
  | "incompatible"
  | "malformed"
  | "error-before-open"
  | "close-before-open"
  | "malformed-ready"
  | "send-error"
  | "late-ready"
let scenarios: Scenario[]
let sockets: FakeSocket[]
class FakeSocket extends EventTarget {
  static OPEN = 1
  readyState = 0
  sent: { id: string; method: string; params: { nonce: string } }[] = []
  readonly nonce: string
  readonly scenario: Scenario
  constructor(readonly url: string) {
    super()
    this.nonce = `nonce-${sockets.length}`
    this.scenario = scenarios.shift() ?? "ready"
    sockets.push(this)
    if (this.scenario === "no-open") return
    setTimeout(() => {
      if (this.scenario === "error-before-open") {
        this.dispatchEvent(new Event("error"))
        return
      }
      if (this.scenario === "close-before-open") {
        this.close(1012)
        return
      }
      this.readyState = 1
      this.dispatchEvent(new Event("open"))
      if (this.scenario === "no-challenge") return
      this.message({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload:
          this.scenario === "malformed"
            ? {}
            : {
                minVersion: this.scenario === "incompatible" ? 2 : 1,
                maxVersion: this.scenario === "incompatible" ? 3 : 1,
                nonce: this.nonce,
                expiresAt: 2_000_000_000,
              },
      })
    }, 0)
  }
  message(value: unknown) {
    this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(value) }))
  }
  send(raw: string) {
    const frame = JSON.parse(raw)
    this.sent.push(frame)
    if (this.scenario === "timeout") return
    if (this.scenario === "send-error") throw new Error("transport failed")
    if (this.scenario === "late-ready") {
      setTimeout(
        () => this.message({ type: "res", id: frame.id, ok: true, payload: {} }),
        100,
      )
      this.close(1006)
      return
    }
    if (this.scenario === "malformed-ready") {
      this.message({ type: "res", id: frame.id, ok: true, payload: {} })
      return
    }
    if (this.scenario === "drop") return this.close(1006)
    if (this.scenario === "auth-close") return this.close(4001)
    if (this.scenario === "denied") {
      this.message({
        type: "res",
        id: frame.id,
        ok: false,
        error: { code: "unauthorized", message: "Invalid credential" },
      })
      return
    }
    this.message({
      type: "res",
      id: frame.id,
      ok: true,
      payload: {
        version: 1,
        gatewayId: "gateway",
        audienceId: "gateway",
        principalId: "reader",
        organizationId: "org",
        membershipId: "membership",
        credentialId: "credential",
        expiresAt: 2_000_000_000,
        grants: [],
        methods: ["auth.session"],
      },
    })
  }
  close(code = 1000) {
    this.readyState = 3
    this.dispatchEvent(Object.assign(new Event("close"), { code, reason: "" }))
  }
}

const options = {
  profile: "product" as const,
  role: "surface" as const,
  surface: { kind: "cli" as const, instance: "test" },
  client: { id: "test", version: "1", platform: "node" as const },
  auth: { credential: "never-print-this-secret" },
}

beforeEach(() => {
  scenarios = []
  sockets = []
  vi.useFakeTimers()
  vi.stubGlobal("WebSocket", FakeSocket)
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

async function connect(
  retry?: import("../application/connect-retry.js").ProductConnectRetryOptions,
) {
  const result = Promise.resolve()
    .then(() =>
      NessaClient.connect({ ...options, config: new NessaClientConfig({ retry }) }),
    )
    .then(
      (client) => ({ client, error: undefined }),
      (error: Error) => ({ client: undefined, error }),
    )
  await vi.runAllTimersAsync()
  return result
}

describe("product connection recovery", () => {
  it.each([
    "no-open",
    "no-challenge",
    "drop",
    "timeout",
    "error-before-open",
    "close-before-open",
    "send-error",
    "late-ready",
  ] as Scenario[])("recovers from %s with a fresh socket and nonce", async (scenario) => {
    scenarios = [scenario, "ready"]
    const { client, error } = await connect()
    expect(error).toBeUndefined()
    expect(client?.profile).toBe("product")
    expect(sockets).toHaveLength(2)
    expect(sockets[0].readyState).toBe(3)
    expect(sockets[1].sent[0].params.nonce).toBe("nonce-1")
    expect(sockets.every((socket) => socket.url.endsWith("/session"))).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
    client?.close()
  })

  it("stops after three attempts and cleans up all timers and sockets", async () => {
    scenarios = ["no-challenge", "no-challenge", "no-challenge"]
    const { error } = await connect()
    expect(error?.message).toBe("session.challenge timeout")
    expect(sockets).toHaveLength(3)
    expect(sockets.every((socket) => socket.readyState === 3)).toBe(true)
    expect(vi.getTimerCount()).toBe(0)
  })

  it.each([
    "denied",
    "auth-close",
    "incompatible",
    "malformed",
    "malformed-ready",
  ] as Scenario[])("does not retry %s", async (scenario) => {
    scenarios = [scenario]
    const { error } = await connect()
    expect(error).toBeInstanceOf(Error)
    expect(error?.message).not.toContain(options.auth.credential)
    expect(sockets).toHaveLength(1)
    expect(sockets[0].readyState).toBe(3)
    expect(vi.getTimerCount()).toBe(0)
    if (scenario === "incompatible") {
      expect(error).toBeInstanceOf(NessaProtocolCompatibilityError)
      expect(error).toMatchObject({
        code: "protocol_incompatible",
        clientMinVersion: 1,
        clientMaxVersion: 1,
        serverMinVersion: 2,
        serverMaxVersion: 3,
      })
      expect(error?.message).toContain("Client supports 1–1; gateway supports 2–3")
      expect(sockets[0].sent).toEqual([])
    }
  })

  it("settles 250 concurrent sessions with a repeatable mixed-failure schedule and no leftover timers", async () => {
    let seed = 0x12345678
    const failures: Scenario[] = [
      "no-open",
      "no-challenge",
      "drop",
      "timeout",
      "denied",
      "auth-close",
      "incompatible",
      "malformed",
      "error-before-open",
      "close-before-open",
      "malformed-ready",
      "send-error",
      "late-ready",
    ]
    scenarios = Array.from({ length: 250 }, () => {
      seed ^= seed << 13
      seed ^= seed >>> 17
      seed ^= seed << 5
      return failures[(seed >>> 0) % failures.length]
    })
    const permanent = scenarios.filter((scenario) =>
      ["denied", "auth-close", "incompatible", "malformed", "malformed-ready"].includes(
        scenario,
      ),
    ).length
    const result = Promise.all(
      Array.from({ length: 250 }, () =>
        NessaClient.connect(options).then(
          (client) => {
            client.close()
            return "ready"
          },
          () => "rejected",
        ),
      ),
    )
    await vi.runAllTimersAsync()
    const values = await result
    expect(values.filter((value) => value === "rejected")).toHaveLength(permanent)
    expect(values.filter((value) => value === "ready")).toHaveLength(250 - permanent)
    expect(sockets).toHaveLength(500 - permanent)
    expect(sockets.every((socket) => socket.readyState === 3)).toBe(true)
    for (const socket of sockets) {
      expect(socket.sent.length).toBeLessThanOrEqual(1)
      if (socket.sent.length) expect(socket.sent[0].params.nonce).toBe(socket.nonce)
    }
    expect(vi.getTimerCount()).toBe(0)
  })

  it("honors a caller's attempt limit, including disabling retries", async () => {
    for (const maxAttempts of [1, 4]) {
      sockets = []
      scenarios = Array.from({ length: maxAttempts }, () => "drop")
      const { error } = await connect({ maxAttempts, initialDelayMs: 0, maxDelayMs: 0 })
      expect(error).toBeInstanceOf(Error)
      expect(sockets).toHaveLength(maxAttempts)
      expect(vi.getTimerCount()).toBe(0)
    }
  })

  it.each([
    { maxAttempts: 0 },
    { maxAttempts: Infinity },
    { maxAttempts: 1.5 },
    { initialDelayMs: -1 },
    { initialDelayMs: NaN },
    { maxDelayMs: 2_147_483_648 },
    { initialDelayMs: 500, maxDelayMs: 100 },
  ])("rejects invalid retry configuration before opening a socket: %j", async (retry) => {
    const { error } = await connect(retry)
    expect(error?.message).toContain("retry.")
    expect(sockets).toHaveLength(0)
  })

  it("honors the backoff cap and disabling jitter", async () => {
    const delays: number[] = []
    let attempts = 0
    const policy = resolveConnectRetry({
      maxAttempts: 4,
      initialDelayMs: 100,
      maxDelayMs: 150,
      jitter: false,
    })
    const random = vi.fn()
    await retryProductConnection(
      async () => {
        if (++attempts < 4) throw new RetryableConnectError("temporary")
      },
      {
        wait: async (ms) => {
          delays.push(ms)
        },
        random,
      },
      policy,
    )
    expect(delays).toEqual([100, 150, 150])
    expect(random).not.toHaveBeenCalled()
  })

  it("backs off with bounded jitter between attempts", async () => {
    const delays: number[] = []
    let attempts = 0
    await expect(
      retryProductConnection(
        async () => {
          if (++attempts < 3) throw new RetryableConnectError("temporary failure")
          return "ready"
        },
        {
          random: () => 0.5,
          wait: async (ms) => {
            delays.push(ms)
          },
        },
      ),
    ).resolves.toBe("ready")
    expect(delays).toEqual([187.5, 375])
  })
})
