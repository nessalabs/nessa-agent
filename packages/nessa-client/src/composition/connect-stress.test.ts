import { NessaClientConfig } from "../application/client-config.js"
import { afterEach, expect, it, vi } from "vitest"
import { type AddressInfo } from "node:net"
import { WebSocket, WebSocketServer } from "ws"
import { NessaClient } from "../presentation/nessa-client.js"

let server: WebSocketServer | undefined
const clients: NessaClient[] = []
afterEach(async () => {
  for (const client of clients.splice(0)) client.close()
  if (server) {
    for (const socket of server.clients) socket.terminate()
    await new Promise<void>((resolve) => server!.close(() => resolve()))
    server = undefined
  }
  vi.unstubAllGlobals()
})

it("cleans up 80 concurrent real connections through repeated drops and auth rejection", async () => {
  vi.stubGlobal("WebSocket", WebSocket)
  server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await new Promise<void>((resolve) => server!.once("listening", resolve))
  const attempts = new Map<number, number>()
  const nonces = new Set<string>()
  const acceptedNonces: string[] = []
  const unexpectedMethods: string[] = []
  let closed = 0
  server.on("connection", (socket, request) => {
    const id = Number(new URL(request.url!, "http://local").searchParams.get("case"))
    const attempt = (attempts.get(id) ?? 0) + 1
    attempts.set(id, attempt)
    socket.on("close", () => {
      closed++
    })
    // Two failures per recovering client, at distinct points in the handshake.
    if (id % 4 === 0 && attempt < 3) {
      socket.close(1012, "restart")
      return
    }
    const nonce = `${id}-${attempt}`
    nonces.add(nonce)
    const challenge = JSON.stringify({
      type: "event",
      event: "session.challenge",
      seq: 1,
      stateVersion: 0,
      payload: {
        minVersion: 1,
        maxVersion: 1,
        nonce,
        expiresAt: 2_000_000_000,
      },
    })
    socket.send(challenge)
    socket.send(challenge) // A duplicate must not cause a second authentication.
    socket.on("message", (raw) => {
      const frame = JSON.parse(String(raw))
      if (frame.method !== "session.authenticate") unexpectedMethods.push(frame.method)
      acceptedNonces.push(frame.params.nonce)
      if (frame.params.nonce !== nonce) {
        socket.close(4001)
        return
      }
      if (id % 4 === 1 && attempt < 3) {
        socket.terminate()
        return
      }
      if (id % 4 === 2) {
        socket.send(
          JSON.stringify({
            type: "res",
            id: frame.id,
            ok: false,
            error: { code: "unauthorized", message: "Invalid credential" },
          }),
        )
        socket.close(4001)
        return
      }
      socket.send(
        JSON.stringify({
          type: "res",
          id: frame.id,
          ok: true,
          payload: {
            version: 1,
            gatewayId: "gateway",
            audienceId: "gateway",
            principalId: `reader-${id}`,
            organizationId: "org",
            membershipId: `membership-${id}`,
            credentialId: `credential-${id}`,
            expiresAt: 2_000_000_000,
            grants: [],
            methods: ["auth.session"],
          },
        }),
      )
    })
  })
  const port = (server.address() as AddressInfo).port
  const results = await Promise.all(
    Array.from({ length: 80 }, async (_, id) => {
      try {
        const client = await NessaClient.connect({
          profile: "product",
          url: `ws://127.0.0.1:${port}?case=${id}`,
          role: "surface",
          surface: { kind: "cli", instance: `${id}` },
          client: { id: `${id}`, version: "1", platform: "node" },
          auth: { credential: `secret-${id}` },
          config: new NessaClientConfig({
            retry: { maxAttempts: 3, initialDelayMs: 5, maxDelayMs: 10 },
          }),
        })
        clients.push(client)
        expect(client.productSession.principalId).toBe(`reader-${id}`)
        client.close()
        client.close() // Closing twice must not produce new work.
        return "ready"
      } catch (error) {
        expect(id % 4).toBe(2)
        expect(error).toMatchObject({ code: "unauthorized" })
        return "denied"
      }
    }),
  )
  expect(results.filter((value) => value === "ready")).toHaveLength(60)
  expect(results.filter((value) => value === "denied")).toHaveLength(20)
  for (let id = 0; id < 80; id++) expect(attempts.get(id)).toBe(id % 4 < 2 ? 3 : 1)
  expect(unexpectedMethods).toEqual([])
  expect(new Set(acceptedNonces).size).toBe(acceptedNonces.length)
  expect(acceptedNonces.every((nonce) => nonces.has(nonce))).toBe(true)
  // Assert cleanup before the teardown safety net terminates anything.
  await vi.waitFor(() => expect(server!.clients.size).toBe(0), { timeout: 2000 })
  expect(closed).toBe(160)
}, 15_000)

it("closes a real socket when the server accepts TCP but never completes the WebSocket upgrade", async () => {
  const { createServer } = await import("node:http")
  const stalled = createServer()
  const peers = new Set<import("node:stream").Duplex>()
  let ended = 0
  stalled.on("upgrade", (_request, socket) => {
    peers.add(socket)
    socket.on("close", () => peers.delete(socket))
    socket.on("end", () => {
      ended++
      socket.destroy()
    })
    socket.resume()
    // Deliberately send no upgrade response.
  })
  await new Promise<void>((resolve) => stalled.listen(0, "127.0.0.1", resolve))
  vi.stubGlobal("WebSocket", WebSocket)
  try {
    const error = await NessaClient.connect({
      profile: "product",
      url: `ws://127.0.0.1:${(stalled.address() as AddressInfo).port}`,
      role: "surface",
      surface: { kind: "cli", instance: "stalled" },
      client: { id: "stalled", version: "1", platform: "node" },
      auth: { credential: "secret" },
      config: new NessaClientConfig({ retry: { maxAttempts: 1 } }),
    }).then(
      () => undefined,
      (error) => error,
    )
    expect(error?.message).toBe("session.challenge timeout")
    await vi.waitFor(() => expect(peers.size).toBe(0), { timeout: 2000 })
    expect(ended).toBe(1)
  } finally {
    for (const socket of peers) socket.destroy()
    await new Promise<void>((resolve) => stalled.close(() => resolve()))
  }
}, 10_000)

it("reauthenticates 40 live clients after dropped RPCs without replaying those RPCs", async () => {
  vi.stubGlobal("WebSocket", WebSocket)
  server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await new Promise<void>((resolve) => server!.once("listening", resolve))
  let connections = 0,
    requests = 0,
    authenticated = 0
  const seen = new Set<string>()
  server.on("connection", (socket) => {
    const nonce = `fresh-${++connections}`
    socket.send(
      JSON.stringify({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload: { minVersion: 1, maxVersion: 1, nonce, expiresAt: 2_000_000_000 },
      }),
    )
    socket.on("message", (raw) => {
      const frame = JSON.parse(String(raw))
      if (frame.method === "session.authenticate") {
        expect(frame.params.nonce).toBe(nonce)
        expect(seen.has(nonce)).toBe(false)
        seen.add(nonce)
        authenticated++
        socket.send(
          JSON.stringify({
            type: "res",
            id: frame.id,
            ok: true,
            payload: {
              version: 1,
              gatewayId: "gateway",
              audienceId: "gateway",
              principalId: "reader",
              organizationId: "org",
              membershipId: "member",
              credentialId: "cred",
              expiresAt: 2_000_000_000,
              grants: [],
              methods: ["server.health"],
            },
          }),
        )
      } else {
        requests++
        socket.terminate()
      }
    })
  })
  const port = (server.address() as AddressInfo).port
  await Promise.all(
    Array.from({ length: 40 }, async (_, id) => {
      const client = await NessaClient.connect({
        profile: "product",
        url: `ws://127.0.0.1:${port}`,
        role: "surface",
        surface: { kind: "cli", instance: `${id}` },
        client: { id: `${id}`, version: "1", platform: "node" },
        auth: { credential: "fixture" },
        config: new NessaClientConfig({
          reconnect: { initialDelayMs: 1, maxDelayMs: 5 },
        }),
      })
      clients.push(client)
      const recovered = new Promise<void>((resolve) => {
        let interrupted = false
        client.onConnectionStateChange((state) => {
          if (state.status === "reconnecting") interrupted = true
          if (state.status === "connected" && interrupted) resolve()
        })
      })
      await expect(client.server.health()).rejects.toMatchObject({
        code: 1006,
        retryable: true,
      })
      await recovered
      expect(client.productSession.credentialId).toBe("cred")
      client.close()
    }),
  )
  expect(connections).toBe(80)
  expect(authenticated).toBe(80)
  expect(requests).toBe(40)
}, 10_000)
