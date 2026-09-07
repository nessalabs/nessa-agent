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
    wss.on("connection", (socket, request) => {
      let seq = 0
      let connected = false
      const product = request.url === "/session"

      seq += 1
      socket.send(
        JSON.stringify({
          type: "event",
          event: "session.challenge",
          payload: {
            minVersion: 1,
            maxVersion: 1,
            nonce: CHALLENGE_NONCE,
            expiresAt: 2_000_000_000,
          },
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
            credential?: string
            nonce?: string
            text?: string
          }
        }

        if (frame.method === "session.authenticate") {
          if (
            !product ||
            frame.params?.nonce !== CHALLENGE_NONCE ||
            frame.params?.credential !== TOKEN
          ) {
            socket.send(
              JSON.stringify({
                type: "res",
                id: frame.id,
                ok: false,
                error: { code: "unauthorized", message: "invalid credential" },
              }),
            )
            socket.close()
            return
          }
          connected = true
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: {
                version: 1,
                gatewayId: "gateway-1",
                principalId: "principal-1",
                organizationId: "organization-1",
                membershipId: "membership-1",
                credentialId: "credential-1",
                audienceId: "gateway-1",
                expiresAt: 2_000_000_000,
                grants: [
                  {
                    action: "credential.manage",
                    resource: { organizationId: "organization-1", id: "gateway-1" },
                  },
                ],
                methods: [
                  "auth.session",
                  "server.health",
                  "credential.issue",
                  "credential.list",
                  "credential.revoke",
                ],
                additiveFutureField: true,
              },
            }),
          )
          return
        }

        const credential = {
          id: "issued-1",
          principalId: "agent-1",
          organizationId: "organization-1",
          audienceId: "gateway-1",
          issuedAt: 1_900_000_000,
          expiresAt: 1_900_086_400,
          revokedAt: null,
          grants: [
            {
              action: "server.read",
              resource: { organizationId: "organization-1", id: "gateway-1" },
            },
          ],
        }
        if (product && frame.method === "credential.issue") {
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: { credential, secret: "issued-secret" },
            }),
          )
          return
        }
        if (product && frame.method === "auth.session") {
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: {
                version: 1,
                gatewayId: "gateway-1",
                principalId: "principal-1",
                organizationId: "organization-1",
                membershipId: "membership-1",
                credentialId: "credential-1",
                audienceId: "gateway-1",
                expiresAt: 2_000_000_000,
                grants: [],
                methods: ["auth.session", "server.health"],
              },
            }),
          )
          return
        }
        if (product && frame.method === "credential.list") {
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: { credentials: [credential] },
            }),
          )
          return
        }
        if (product && frame.method === "credential.revoke") {
          socket.send(
            JSON.stringify({
              type: "res",
              id: frame.id,
              ok: true,
              payload: { credentialId: "issued-1", revision: 2 },
            }),
          )
          return
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
          const text = typeof frame.params?.text === "string" ? frame.params.text : ""
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

  it("connects, completes handshake, and calls server.health", async () => {
    const client = await NessaClient.connect({
      stage: "ci",
      url: `ws://127.0.0.1:${port}`,
      role: "surface",
      surface: { kind: "panel", instance: "test" },
      client: { id: "test-client", version: "0.1.0", platform: "node" },
      auth: { credential: TOKEN },
    })

    expect(client.productSession.gatewayId).toBe("gateway-1")

    const health = await client.server.health()
    expect(health).toEqual({
      ok: true,
      runtimeStatus: "ready",
      uptimeMs: 42,
    })

    const echo = await client.conversation.echo("hey")
    expect(echo).toEqual({ text: "hey" })

    client.close()
  })

  it("rejects invalid credentials with NessaRpcError", async () => {
    try {
      await NessaClient.connect({
        stage: "ci",
        url: `ws://127.0.0.1:${port}`,
        role: "surface",
        surface: { kind: "panel", instance: "test" },
        client: { id: "test-client", version: "0.1.0", platform: "node" },
        auth: { credential: "wrong" },
      })
      expect.unreachable("expected connect to reject")
    } catch (error) {
      expect(error).toBeInstanceOf(NessaRpcError)
      expect((error as NessaRpcError).code).toBe("unauthorized")
      expect((error as NessaRpcError).message).toBe("invalid credential")
    }
  })

  it("uses /session and requires a successful product handshake", async () => {
    const client = await NessaClient.connect({
      profile: "product",
      stage: "ci",
      url: `ws://127.0.0.1:${port}`,
      role: "surface",
      surface: { kind: "cli", instance: "admin" },
      client: { id: "admin-cli", version: "0.1.0", platform: "node" },
      auth: { credential: TOKEN },
    })

    expect(client.profile).toBe("product")
    expect(client.productSession).toMatchObject({
      principalId: "principal-1",
      organizationId: "organization-1",
      expiresAt: 2_000_000_000,
      methods: expect.arrayContaining(["server.health", "credential.issue"]),
    })
    expect(await client.auth.session()).toMatchObject({
      credentialId: "credential-1",
      methods: ["auth.session", "server.health"],
    })
    const issue = await client.credentials.issue({
      requestId: "command-1",
      principal: { id: "agent-1", kind: "agent" },
      membership: {
        id: "membership-agent-1",
        principalId: "agent-1",
        organizationId: "organization-1",
        role: "member",
        state: "active",
      },
      expiresAt: 1_900_086_400,
      grants: [
        {
          action: "server.read",
          resource: { organizationId: "organization-1", id: "gateway-1" },
        },
      ],
    })
    expect(issue).toMatchObject({
      credential: { id: "issued-1" },
      secret: "issued-secret",
    })
    expect(await client.credentials.list()).toMatchObject({
      credentials: [{ id: "issued-1" }],
    })
    expect(await client.credentials.revoke("issued-1", "command-2")).toEqual({
      requestId: "command-2",
      credentialId: "issued-1",
      revision: 2,
    })
    client.close()
  })

  it("closes and rejects a failed product handshake without exposing the credential", async () => {
    await expect(
      NessaClient.connect({
        profile: "product",
        stage: "ci",
        url: `ws://127.0.0.1:${port}`,
        role: "surface",
        surface: { kind: "cli", instance: "admin" },
        client: { id: "admin-cli", version: "0.1.0", platform: "node" },
        auth: { credential: "private-wrong-value" },
      }),
    ).rejects.toMatchObject({ code: "unauthorized", message: "invalid credential" })
  })
})
