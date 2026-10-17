import { afterEach, expect, it, vi } from "vitest"
import type { AddressInfo } from "node:net"
import { WebSocket, WebSocketServer } from "ws"
import sampleView from "../../../../protocol/product/samples/conversation-read.json"
import {
  createConversationReadTools,
  NessaClient,
  NessaClientConfig,
  NessaConnectionClosedError,
  NessaRpcError,
  NessaSessionUnavailableError,
} from "../index.js"

let server: WebSocketServer | undefined
let client: NessaClient | undefined
afterEach(async () => {
  client?.close()
  client = undefined
  if (server) {
    for (const socket of server.clients) socket.terminate()
    await new Promise<void>((resolve) => server!.close(() => resolve()))
    server = undefined
  }
  vi.unstubAllGlobals()
})

// This fixture implements the product socket, not the gateway's Cedar authority.
// The existing gateway suites own current credential/membership/Cedar decisions.
it("reads synthetic gateway data, preserves denial, and reauthenticates without replay", async () => {
  vi.stubGlobal("WebSocket", WebSocket)
  server = new WebSocketServer({ host: "127.0.0.1", port: 0 })
  await new Promise<void>((resolve) => server!.once("listening", resolve))
  let connections = 0
  let revoked = false
  let drop = false
  let releaseAuthentication: (() => void) | undefined
  const requests: { method: string; params: unknown }[] = []
  const credentials: string[] = []
  const view = { ...sampleView, truncated: true }
  const listed = {
    complete: false,
    conversations: [
      {
        conversationId: sampleView.conversationId,
        title: "Synthetic gateway fixture",
        preview: "Synthetic text only",
        createdAtMs: 1,
        updatedAtMs: 2,
        running: false,
        archived: false,
      },
    ],
  }
  server.on("connection", (socket) => {
    const nonce = `fixture-${++connections}`
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
      const respond = (payload: unknown) =>
        socket.send(
          JSON.stringify({
            type: "res",
            id: frame.id,
            ok: true,
            payload,
          }),
        )
      if (frame.method === "session.authenticate") {
        expect(frame.params.nonce).toBe(nonce)
        credentials.push(frame.params.credential)
        const authenticate = () =>
          respond({
            version: 1,
            gatewayId: "gateway",
            audienceId: "gateway",
            principalId: "reader",
            organizationId: "org",
            membershipId: "member",
            credentialId: "fixture-credential",
            expiresAt: 2_000_000_000,
            grants: [],
            methods: ["conversation.list", "conversation.read"],
          })
        if (connections === 1) authenticate()
        else releaseAuthentication = authenticate
        return
      }
      requests.push({ method: frame.method, params: frame.params })
      if (revoked) {
        socket.send(
          JSON.stringify({
            type: "res",
            id: frame.id,
            ok: false,
            error: {
              code: "forbidden",
              message: "Fixture authority was revoked",
              details: { fixture: true },
            },
          }),
        )
      } else if (drop) {
        drop = false
        socket.terminate()
      } else if (frame.method === "conversation.list") respond(listed)
      else if (frame.method === "conversation.read") respond(view)
      else throw new Error(`Unexpected fixture method ${frame.method}`)
    })
  })
  client = await NessaClient.connect({
    profile: "product",
    stage: "ci",
    role: "surface",
    url: `ws://127.0.0.1:${(server.address() as AddressInfo).port}/session`,
    surface: { kind: "cli", instance: "read-tool-fixture" },
    client: { id: "read-tool-fixture", version: "1", platform: "node" },
    auth: { credential: "synthetic-fixture-only" },
    config: new NessaClientConfig({
      reconnect: { initialDelayMs: 1, maxDelayMs: 5 },
    }),
  })
  const tools = createConversationReadTools(client)
  await expect(tools.call("nessa_conversations_list", {})).resolves.toEqual(listed)
  await expect(
    tools.call("nessa_conversation_read", {
      conversationId: sampleView.conversationId,
    }),
  ).resolves.toEqual(view)
  revoked = true
  await expect(
    tools.call("nessa_conversation_read", {
      conversationId: sampleView.conversationId,
    }),
  ).rejects.toBeInstanceOf(NessaRpcError)
  await expect(tools.call("nessa_conversations_list", {})).rejects.toMatchObject({
    code: "forbidden",
    details: { fixture: true },
  })
  expect(requests).toHaveLength(4)
  revoked = false
  drop = true
  await expect(tools.call("nessa_conversations_list", {})).rejects.toBeInstanceOf(
    NessaConnectionClosedError,
  )
  await vi.waitFor(() => expect(releaseAuthentication).toBeTypeOf("function"))
  await expect(tools.call("nessa_conversations_list", {})).rejects.toBeInstanceOf(
    NessaSessionUnavailableError,
  )
  expect(requests).toHaveLength(5)
  releaseAuthentication!()
  await vi.waitFor(() => expect(client!.connectionState.status).toBe("connected"))
  expect(requests).toHaveLength(5)
  await expect(tools.call("nessa_conversations_list", {})).resolves.toEqual(listed)
  expect(requests).toEqual([
    { method: "conversation.list", params: {} },
    {
      method: "conversation.read",
      params: { conversationId: sampleView.conversationId },
    },
    {
      method: "conversation.read",
      params: { conversationId: sampleView.conversationId },
    },
    { method: "conversation.list", params: {} },
    { method: "conversation.list", params: {} },
    { method: "conversation.list", params: {} },
  ])
  expect(connections).toBe(2)
  expect(credentials).toEqual(["synthetic-fixture-only", "synthetic-fixture-only"])
  client.close()
  await expect(tools.call("nessa_conversations_list", {})).rejects.toBeInstanceOf(
    NessaSessionUnavailableError,
  )
  expect(requests).toHaveLength(6)
}, 10_000)
