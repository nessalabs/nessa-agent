import { describe, expect, it, vi } from "vitest"

import {
  connectBrowserSession,
  connectDevSession,
  SessionHealthError,
} from "./dev-session"
import { createSessionHandle } from "./handle"

describe("connectDevSession", () => {
  it("closes the client when health fails after connect", async () => {
    const close = vi.fn()
    const connect = vi.fn().mockResolvedValue({
      productSession: { version: 1 },
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

  it("returns the client when product authentication and health succeed", async () => {
    const close = vi.fn()
    const hello = {
      version: 1,
      gatewayId: "gateway",
      principalId: "panel",
      organizationId: "org",
      membershipId: "panel-member",
      credentialId: "panel-token",
      audienceId: "gateway",
      expiresAt: null,
      grants: [],
      methods: ["server.health"],
    }
    const health = { ok: true, runtimeStatus: "ready", uptimeMs: 1 }
    const ping = vi.fn().mockImplementation(async (nonce: string) => ({
      ok: true as const,
      nonce,
    }))
    const client = {
      productSession: hello,
      server: { health: vi.fn().mockResolvedValue(health), ping },
      close,
    }
    const connect = vi.fn().mockResolvedValue(client)

    const established = await connectDevSession({ connect })
    expect(established.client).toBe(client)
    expect(established.hello).toEqual(hello)
    expect(established.health).toEqual(health)
    expect(ping).not.toHaveBeenCalled()
    expect(connect).toHaveBeenCalledWith(
      expect.objectContaining({
        profile: "product",
        client: expect.objectContaining({ id: "nessa-panel" }),
      }),
    )
    expect(close).not.toHaveBeenCalled()
  })

  it("passes the configured credential source without supplying a dev token", async () => {
    const source = { load: vi.fn(async () => "private") }
    const endpointSource = { load: vi.fn(async () => "ws://127.0.0.1:9137") }
    const connect = vi.fn().mockResolvedValue({
      productSession: {},
      server: { health: vi.fn(async () => ({})) },
      close: vi.fn(),
    })
    await connectDevSession({
      connect,
      credentialSource: source,
      endpointSource,
      stage: "ci",
    })
    expect(connect).toHaveBeenCalledWith(
      expect.objectContaining({
        profile: "product",
        stage: "ci",
        credentialSource: source,
        endpointSource,
      }),
    )
    expect(connect.mock.calls[0][0].auth).toBeUndefined()
  })

  it("uses an explicit gateway origin without consulting endpoint discovery", async () => {
    const endpointSource = { load: vi.fn(async () => "ws://127.0.0.1:9137") }
    const connect = vi.fn().mockResolvedValue({
      productSession: {},
      server: { health: vi.fn(async () => ({})) },
      close: vi.fn(),
    })

    await connectDevSession({
      connect,
      gatewayBaseUrl: "https://127.0.0.1:42177",
      endpointSource,
    })

    expect(connect).toHaveBeenCalledWith(
      expect.objectContaining({ url: "wss://127.0.0.1:42177" }),
    )
    expect(connect.mock.calls[0][0].endpointSource).toBeUndefined()
  })

  it("keeps browser-cookie sessions on their supplied browser URL", async () => {
    const connect = vi.fn().mockResolvedValue({
      productSession: {},
      server: { health: vi.fn(async () => ({})) },
      close: vi.fn(),
    })

    await connectDevSession({
      connect,
      browserUrl: "ws://127.0.0.1:1420/browser/session",
      gatewayBaseUrl: "http://127.0.0.1:42177",
    })

    expect(connect).toHaveBeenCalledWith(
      expect.objectContaining({
        url: "ws://127.0.0.1:1420/browser/session",
        auth: { browserCookie: true },
      }),
    )
  })
})

describe("session client handle", () => {
  it("stores and clears the live client outside Redux", () => {
    const fake = { close: vi.fn() } as never
    const handle = createSessionHandle()
    handle.set(fake)
    expect(handle.get()).toBe(fake)
    handle.set(null)
    expect(handle.get()).toBeNull()
  })
})

describe("connectBrowserSession", () => {
  it("refuses an origin that is not signed in, before anything is opened", async () => {
    const connect = vi.fn()
    await expect(
      connectBrowserSession({
        auth: { restore: () => Promise.resolve(false) },
        stage: "dev",
        clientId: "nessa-browser",
        pageUrl: "http://127.0.0.1:1420/desktop.html?gateway",
        connect,
      }),
    ).rejects.toMatchObject({ code: "unauthorized" })
    expect(connect).not.toHaveBeenCalled()
  })

  it("connects over this origin's browser session, as the surface it names", async () => {
    const connect = vi.fn().mockResolvedValue({
      productSession: { version: 1 },
      server: { health: vi.fn().mockResolvedValue({ ok: true }) },
      close: vi.fn(),
    })
    await connectBrowserSession({
      auth: { restore: () => Promise.resolve(true) },
      stage: "dev",
      clientId: "nessa-browser",
      surfaceKind: "desktop",
      pageUrl: "http://127.0.0.1:1420/desktop.html?gateway",
      connect,
    })
    expect(connect).toHaveBeenCalledWith(
      expect.objectContaining({
        url: "ws://127.0.0.1:1420/browser/session",
        auth: { browserCookie: true },
        surface: expect.objectContaining({ kind: "desktop" }),
        client: expect.objectContaining({ id: "nessa-browser" }),
      }),
    )
  })
})
