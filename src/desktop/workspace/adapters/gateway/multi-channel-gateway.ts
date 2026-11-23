/**
 * Composite `ChannelSource` across 1..N gateways (ADR 1417, #1419).
 *
 * Routes channel creation to `draft.gatewayId ?? LOCAL_GATEWAY_ID` and channel
 * placement/coordinator mutations to the channel's owning gateway, qualifying
 * returned channel IDs at the boundary without cross-gateway fallback (G1, G2, G10).
 */
import {
  LOCAL_GATEWAY_ID,
  parseQualifiedId,
  qualifyEntityId,
  type GatewayConnectionState,
} from "../../model/gateway-id"
import type { ChannelDraft } from "../../model/channel-draft"
import type { Channel } from "../../model/workspace-index"
import {
  ChannelCreationError,
  type ChannelPlacement,
  type ChannelSource,
} from "../../application/ports"

export interface ManagedChannelGateway {
  readonly gatewayId: string
  readonly status: GatewayConnectionState
  readonly channels: ChannelSource
}

export interface MultiGatewayChannelBacking {
  entry(gatewayId: string): ManagedChannelGateway | undefined
}

function qualifyChannel(gatewayId: string, channel: Channel): Channel {
  if (gatewayId === LOCAL_GATEWAY_ID) {
    return { ...channel, gatewayId: LOCAL_GATEWAY_ID }
  }
  return {
    ...channel,
    id: qualifyEntityId(gatewayId, channel.id),
    sectionId: qualifyEntityId(gatewayId, channel.sectionId),
    gatewayId,
    coordinatorConversationId:
      channel.coordinatorConversationId !== undefined
        ? qualifyEntityId(gatewayId, channel.coordinatorConversationId)
        : undefined,
  }
}

/**
 * Create a composite `ChannelSource` backed by all gateways in `backing`.
 */
export function multiChannelGateway(backing: MultiGatewayChannelBacking): ChannelSource {
  function resolveGateway(gatewayId: string): ManagedChannelGateway {
    const gateway = backing.entry(gatewayId)
    if (!gateway) {
      throw new ChannelCreationError("unavailable")
    }
    if (
      gateway.gatewayId !== LOCAL_GATEWAY_ID &&
      (gateway.status === "auth-refused" ||
        gateway.status === "disconnected" ||
        gateway.status === "refused")
    ) {
      throw new ChannelCreationError("unavailable")
    }
    return gateway
  }

  return {
    async create(draft: ChannelDraft, requestId: string): Promise<Channel> {
      const targetGatewayId = draft.gatewayId ?? LOCAL_GATEWAY_ID
      const gateway = resolveGateway(targetGatewayId)
      const created = await gateway.channels.create(draft, requestId)
      return qualifyChannel(gateway.gatewayId, created)
    },

    async setPlacement(
      channelId: string,
      placement: ChannelPlacement,
      requestId: string,
    ): Promise<Channel> {
      const { gatewayId, rawId } = parseQualifiedId(channelId)
      const gateway = resolveGateway(gatewayId)
      if (!gateway.channels.setPlacement) {
        throw new ChannelCreationError("unavailable")
      }
      const updated = await gateway.channels.setPlacement(rawId, placement, requestId)
      return qualifyChannel(gateway.gatewayId, updated)
    },

    async setCoordinator(
      channelId: string,
      on: boolean,
      requestId: string,
    ): Promise<Channel> {
      const { gatewayId, rawId } = parseQualifiedId(channelId)
      const gateway = resolveGateway(gatewayId)
      if (!gateway.channels.setCoordinator) {
        throw new ChannelCreationError("unavailable")
      }
      const updated = await gateway.channels.setCoordinator(rawId, on, requestId)
      return qualifyChannel(gateway.gatewayId, updated)
    },
  }
}
