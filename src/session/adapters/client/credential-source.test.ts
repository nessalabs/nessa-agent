import { beforeEach, expect, it, vi } from "vitest"

const host = vi.hoisted(() => ({
  hasNativeHost: vi.fn(() => true),
  loadAssignedGatewayEndpoint: vi.fn(),
  loadAssignedSurfaceCredential: vi.fn(),
}))

vi.mock("../../../host", () => host)

import { nativeGatewayEndpointSource } from "./credential-source"

beforeEach(() => vi.clearAllMocks())

it.each([
  ["prod", 7420],
  ["dev", 7421],
  ["ci", 7420],
  ["alpha", 7420],
] as const)(
  "retains the native %s stage address when no publication is available",
  async (stage, port) => {
    host.loadAssignedGatewayEndpoint.mockResolvedValue(undefined)
    await expect(nativeGatewayEndpointSource()?.load({ stage })).resolves.toBe(
      `ws://127.0.0.1:${port}`,
    )
  },
)

it("uses a verified native publication instead of the stage address", async () => {
  host.loadAssignedGatewayEndpoint.mockResolvedValue("ws://127.0.0.1:9137")
  await expect(nativeGatewayEndpointSource()?.load({ stage: "prod" })).resolves.toBe(
    "ws://127.0.0.1:9137",
  )
})
