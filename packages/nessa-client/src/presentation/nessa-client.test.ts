import { afterAll, beforeAll, describe, expect, it } from "vitest"
import { WebSocket, WebSocketServer } from "ws"

import { NessaClient } from "./nessa-client.js"

const TOKEN = "test-token"
const PORT = 19_420
const CHALLENGE_NONCE = "test-challenge-nonce"

describe("NessaClient", () => {
  let wss: WebSocketServer

  beforeAll(async () => {
    wss = new WebSocketServer({ host: "127.0.0.1", port: PORT })
    wss.on("connection", (socket) => {
      let seq = 0
      let connected = false

      seq += 1
      socket.send(
        JSON.stringify({
          type: "event",
          event: "connect.challenge",
          payload: { nonce: CHALLENGE_NONCE, protocol: 1 },
          seq,
          stateVersion: 0,
        }),
      )

      socket.on("message", (raw) => {
        const frame = JSON.parse(String(raw)) as {
          type?: string
          id?: string
          method?: string
          params?: { auth?: { token?: string; nonce?: string } }
        }
        if (frame.type !== "req" || !frame.id || !frame.method) {
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id ?? "0",
              ok: false,
              error: { code: "invalid_request", message: "expected req frame" },
            }),
          )
          return
        }

        if (frame.method === "connect") {
          if (frame.params?.auth?.nonce !== CHALLENGE_NONCE) {
            socket.send(
              JSON.stringify({
                type: "res",
                id: frame.id,
                ok: false,
                error: { code: "invalid_challenge", message: "bad nonce" },
              }),
            )
            return
          }
          if (frame.params?.auth?.token !== TOKEN) {
            socket.send(
              JSON.stringify({
                type: "res",
                id: frame.id,
                ok: false,
                error: { code: "unauthorized", message: "invalid token" },
              }),
            )
            return
          }
          connected = true
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: {
                protocol: 1,
                scopes: ["server.read"],
                serverVersion: "0.1.0-test",
                runtimeStatus: "ready",
                policy: { maxPayloadBytes: 65536 },
              },
            }),
          )
          return
        }

        if (frame.method === "server.health") {
          if (!connected) {
            socket.send(
              JSON.stringify({
                type: "res",
                id: frame.id,
                ok: false,
                error: { code: "not_connected", message: "connect required" },
              }),
            )
            return
          }
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: {
                ok: true,
                runtimeStatus: "ready",
                uptimeMs: 42,
              },
            }),
          )
          return
        }

        socket.send(
          JSON.stringify({
            type: "res",
            id: frame.id,
            ok: false,
            error: { code: "unknown_method", message: frame.method },
          }),
        )
      })
    })
    await new Promise<void>((resolve) => wss.once("listening", resolve))
  })

  afterAll(() => {
    for (const client of wss.clients) client.terminate()
    wss.close()
  })

  it("connects, completes handshake, and calls server.health", async () => {
    const client = await NessaClient.connect({
      url: `ws://127.0.0.1:${PORT}`,
      role: "surface",
      surface: { kind: "panel", instance: "test" },
      client: { id: "test-client", version: "0.1.0", platform: "node" },
      auth: { token: TOKEN },
    })

    expect(client.session.serverVersion).toBe("0.1.0-test")
    expect(client.session.scopes).toContain("server.read")

    const health = await client.server.health()
    expect(health).toEqual({
      ok: true,
      runtimeStatus: "ready",
      uptimeMs: 42,
    })

    client.close()
  })

  it("rejects invalid auth tokens", async () => {
    await expect(
      NessaClient.connect({
        url: `ws://127.0.0.1:${PORT}`,
        role: "surface",
        surface: { kind: "panel", instance: "test" },
        client: { id: "test-client", version: "0.1.0", platform: "node" },
        auth: { token: "wrong" },
      }),
    ).rejects.toThrow("invalid token")
  })
})

if (typeof globalThis.WebSocket === "undefined") {
  ;(globalThis as typeof globalThis & { WebSocket: typeof WebSocket }).WebSocket =
    WebSocket as unknown as typeof globalThis.WebSocket
}
