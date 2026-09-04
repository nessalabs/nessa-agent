import { afterAll, beforeAll, describe, expect, it } from "vitest"
import { type AddressInfo } from "node:net"
import { WebSocket, WebSocketServer } from "ws"

import { NessaClient } from "./nessa-client.js"
import { NessaRpcError } from "../application/rpc-error.js"

if (typeof globalThis.WebSocket === "undefined") {
  ;(globalThis as typeof globalThis & { WebSocket: typeof WebSocket }).WebSocket =
    WebSocket as unknown as typeof globalThis.WebSocket
}

const TOKEN = "test-token"
const CHALLENGE_NONCE = "test-challenge-nonce"

describe("NessaClient", () => {
  let wss: WebSocketServer
  let port: number

  beforeAll(async () => {
    wss = new WebSocketServer({ host: "127.0.0.1", port: 0 })
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
          params?: {
            auth?: { token?: string; nonce?: string }
            nonce?: string
            text?: string
          }
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
          if (connected) {
            socket.send(
              JSON.stringify({
                type: "res",
                id: frame.id,
                ok: false,
                error: {
                  code: "already_connected",
                  message: "session already completed connect",
                },
              }),
            )
            return
          }
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
                shortcuts: { version: 1, bindings: [] },
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

        if (frame.method === "server.ping") {
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
          const nonce =
            typeof frame.params?.nonce === "string" ? frame.params.nonce : ""
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: { ok: true, nonce },
            }),
          )
          return
        }

        if (frame.method === "conversation.echo") {
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
          const text =
            typeof frame.params?.text === "string" ? frame.params.text : ""
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: { text },
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
    port = (wss.address() as AddressInfo).port
  })

  afterAll(() => {
    for (const client of wss.clients) client.terminate()
    wss.close()
  })

  it("connects, completes handshake, and calls server.health and server.ping", async () => {
    const client = await NessaClient.connect({
      stage: "ci",
      url: `ws://127.0.0.1:${port}`,
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

    const ping = await client.server.ping("probe-1")
    expect(ping).toEqual({ ok: true, nonce: "probe-1" })

    const echo = await client.conversation.echo("hey")
    expect(echo).toEqual({ text: "hey" })

    client.close()
  })

  it("rejects invalid auth tokens with NessaRpcError", async () => {
    try {
      await NessaClient.connect({
        stage: "ci",
        url: `ws://127.0.0.1:${port}`,
        role: "surface",
        surface: { kind: "panel", instance: "test" },
        client: { id: "test-client", version: "0.1.0", platform: "node" },
        auth: { token: "wrong" },
      })
      expect.unreachable("expected connect to reject")
    } catch (error) {
      expect(error).toBeInstanceOf(NessaRpcError)
      expect((error as NessaRpcError).code).toBe("unauthorized")
      expect((error as NessaRpcError).message).toBe("invalid token")
    }
  })
})
