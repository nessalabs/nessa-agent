import { describe, expect, it, vi } from "vitest"
import { LOCAL_GATEWAY_ID, qualifyEntityId } from "../../model/gateway-id"
import { emptyDraft } from "../../model/channel-draft"
import type { Channel } from "../../model/workspace-index"
import type { ChannelSource } from "../../application/ports"
import { multiChannelGateway, type ManagedChannelGateway } from "./multi-channel-gateway"

function fakeChannelSource(prefix: string): {
  source: ChannelSource
  created: string[]
} {
  const created: string[] = []
  const source: ChannelSource = {
    create: vi.fn(async (draft) => {
      created.push(draft.name)
      const channel: Channel = {
        id: `${prefix}-chan`,
        name: draft.name,
        sectionId: "gateway",
        private: false,
        topic: "",
        coordinatorConversationId: "coord-1",
      }
      return channel
    }),
    setCoordinator: vi.fn(async (channelId, on) => ({
      id: channelId,
      name: "general",
      sectionId: "gateway",
      private: false,
      topic: "",
      coordinatorOn: on,
    })),
  }
  return { source, created }
}

describe("multiChannelGateway (G1, G2, G10)", () => {
  const model = { provider: "anthropic", modelId: "claude-sonnet-4-5" }

  it("routes create() to draft.gatewayId, qualifies returned IDs, and fails closed when remote is offline", async () => {
    const local = fakeChannelSource("local")
    const cloud = fakeChannelSource("cloud")
    let cloudStatus: ManagedChannelGateway["status"] = "connected"
    const channels = multiChannelGateway({
      entry: (id) => {
        if (id === LOCAL_GATEWAY_ID) {
          return {
            gatewayId: LOCAL_GATEWAY_ID,
            status: "connected",
            channels: local.source,
          }
        }
        if (id === "cloud-1") {
          return { gatewayId: "cloud-1", status: cloudStatus, channels: cloud.source }
        }
        return undefined
      },
    })

    const remoteCreated = await channels.create(
      { ...emptyDraft(model), name: "infra", gatewayId: "cloud-1" },
      "req-1",
    )
    expect(remoteCreated.id).toBe(qualifyEntityId("cloud-1", "cloud-chan"))
    expect(remoteCreated.coordinatorConversationId).toBe(
      qualifyEntityId("cloud-1", "coord-1"),
    )
    expect(cloud.created).toEqual(["infra"])
    expect(local.created).toHaveLength(0)

    cloudStatus = "disconnected"
    await expect(
      channels.create(
        { ...emptyDraft(model), name: "offline", gatewayId: "cloud-1" },
        "req-2",
      ),
    ).rejects.toMatchObject({ reason: "unavailable" })
    expect(local.created).toHaveLength(0)
  })
})
