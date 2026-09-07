import { NessaClientConfig } from "./client-config.js"
import { afterEach, describe, expect, it, vi } from "vitest"

import { runProductHandshake } from "./product-connect-flow.js"
import type { ResolvedProductConnectOptions } from "./resolve-options.js"
import type { WireSession } from "../transport/wire-session.js"

describe("runProductHandshake", () => {
  afterEach(() => vi.useRealTimers())
  it("bounds the authentication wait by the advertised deadline and rejects expired challenges", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(100_500)
    const request = vi.fn().mockRejectedValue(new Error("sent"))
    const wire = { request } as unknown as WireSession
    const options: ResolvedProductConnectOptions = {
      profile: "product",
      config: new NessaClientConfig(),
      stage: "dev",
      url: "ws://127.0.0.1:7420/session",
      role: "surface",
      surface: { kind: "cli", instance: "test" },
      client: { id: "test", version: "0.1.0", platform: "node" },
      auth: { credential: "secret" },
    }
    const challenge = { minVersion: 1, maxVersion: 1, nonce: "challenge", expiresAt: 102 }
    await expect(runProductHandshake(wire, options, challenge)).rejects.toThrow("sent")
    expect(request.mock.calls[0]?.[2]).toBe(1_500)
    request.mockClear()
    vi.setSystemTime(102_000)
    await expect(runProductHandshake(wire, options, challenge)).rejects.toThrow(
      "session.challenge expired",
    )
    expect(request).not.toHaveBeenCalled()
  })
  it("does not send credential evidence when protocol ranges do not overlap", async () => {
    let requests = 0
    const wire = {
      request: () => {
        requests += 1
        return Promise.resolve({})
      },
    } as unknown as WireSession
    const options: ResolvedProductConnectOptions = {
      profile: "product",
      config: new NessaClientConfig(),
      stage: "dev",
      url: "ws://127.0.0.1:7420/session",
      role: "surface",
      surface: { kind: "cli", instance: "test" },
      client: { id: "test", version: "0.1.0", platform: "node" },
      auth: { credential: "must-not-be-sent" },
      minProtocol: 1,
      maxProtocol: 1,
    }

    await expect(
      runProductHandshake(wire, options, {
        minVersion: 2,
        maxVersion: 3,
        nonce: "challenge",
        expiresAt: 2_000_000_000,
      }),
    ).rejects.toThrow("no compatible product protocol version")
    expect(requests).toBe(0)
  })
})
