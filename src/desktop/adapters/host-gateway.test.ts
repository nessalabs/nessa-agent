import type { CredentialSource, GatewayEndpointSource, NessaClient } from "@nessa/client"
import { expect, it, vi } from "vitest"
import { hostGateway } from "./host-gateway"

const credentialSource: CredentialSource = { load: async () => "fixture-only" }
const endpointSource: GatewayEndpointSource = { load: async () => "ws://127.0.0.1:7421" }

function fakeConnect(health: () => Promise<unknown> = async () => ({ ok: true })) {
  const close = vi.fn()
  const client = { productSession: {}, server: { health }, close }
  const connect = vi.fn(async () => client) as unknown as typeof NessaClient.connect
  return { connect, client, close }
}

it("connects as the desktop window under the panel's credential, over the host's sources (C1)", async () => {
  const { connect, client } = fakeConnect()
  const gateway = hostGateway(
    { stage: "alpha" },
    { connect, credentialSource, endpointSource },
  )

  expect(connect).not.toHaveBeenCalled()
  await expect(gateway()).resolves.toBe(client)
  expect(connect).toHaveBeenCalledTimes(1)
  const options = vi.mocked(connect).mock.calls[0][0]
  expect(options).toMatchObject({
    profile: "product",
    stage: "alpha",
    role: "surface",
    credentialSource,
    endpointSource,
    surface: { kind: "desktop" },
    // The credential's name, which the native source hands out to nothing else.
    client: { id: "nessa-panel" },
  })
  // The host's endpoint decides where; nothing on the page does, and the
  // secret is the source's to hand over at the handshake, never an option here.
  expect(options).not.toHaveProperty("url")
  expect(options).not.toHaveProperty("auth")
})

it("follows the build's gateway override as the panel does", async () => {
  const { connect } = fakeConnect()
  await hostGateway(
    { stage: "dev", gatewayBaseUrlOverride: "http://127.0.0.1:9137" },
    { connect, credentialSource, endpointSource },
  )()
  expect(vi.mocked(connect).mock.calls[0][0]).toMatchObject({
    url: "ws://127.0.0.1:9137",
  })
})

it("each call is a fresh connection, for the source to make again after one closed", async () => {
  const { connect } = fakeConnect()
  const gateway = hostGateway(
    { stage: "dev" },
    { connect, credentialSource, endpointSource },
  )
  await gateway()
  await gateway()
  expect(connect).toHaveBeenCalledTimes(2)
  const [first, second] = vi
    .mocked(connect)
    .mock.calls.map(([options]) => options.surface)
  expect(first?.instance).not.toBe(second?.instance)
})

it("a probe that fails closes the client it connected and rejects", async () => {
  const refused = new Error("probe")
  const { connect, close } = fakeConnect(() => Promise.reject(refused))
  await expect(
    hostGateway({ stage: "dev" }, { connect, credentialSource, endpointSource })(),
  ).rejects.toMatchObject({ name: "SessionHealthError", cause: refused })
  expect(close).toHaveBeenCalledTimes(1)
})
