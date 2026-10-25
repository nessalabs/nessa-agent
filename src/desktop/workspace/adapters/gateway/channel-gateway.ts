/**
 * The `ChannelSource` over the window's gateway client: `client.channels.create`,
 * with the draft read into the protocol's words (the agent from the model's
 * provider, `approvalMode` from its access) and each refusal mapped by a total
 * table, so a code the protocol adds is a type error here rather than a
 * sentence nobody wrote. An answer that never came, a code this build does not
 * know, or an answer the client's own validation refused is `unavailable`, and
 * is logged. The channel it answers is the index's (`channelFrom`).
 */
import { ChannelErrorCode, channelRefusal, type ChannelApi } from "@nessa/client"
import { agentForProvider } from "../../../model/composer-options"
import { ChannelCreationError, type ChannelSource } from "../../application/ports"
import { channelName, type ChannelFailure } from "../../model/channel-draft"
import { channelFrom } from "./gateway-views"

/** What making a channel asks of the window's gateway client. */
export interface ChannelClient {
  readonly channels: Pick<ChannelApi, "create">
}

/** Every channel code, as what it means to the flow. */
const failures: Record<ChannelErrorCode, ChannelFailure> = {
  [ChannelErrorCode.InvalidRequest]: "invalid",
  [ChannelErrorCode.Forbidden]: "forbidden",
  [ChannelErrorCode.ConversationsNotConfigured]: "unavailable",
  [ChannelErrorCode.ChannelFolderNotFound]: "folder-not-found",
  [ChannelErrorCode.ChannelFolderNotADirectory]: "folder-not-a-directory",
  [ChannelErrorCode.ChannelFolderRefused]: "folder-refused",
  [ChannelErrorCode.ChannelResourceNotFound]: "resource-not-found",
  [ChannelErrorCode.ChannelLimitReached]: "limit",
  [ChannelErrorCode.ChannelRequestConflict]: "conflict",
  [ChannelErrorCode.ChannelStorageUnavailable]: "unavailable",
  [ChannelErrorCode.TemporarilyUnavailable]: "unavailable",
}

export function channelGateway(options: {
  readonly connected: () => Promise<ChannelClient>
}): ChannelSource {
  return {
    async create(draft, requestId) {
      const name = channelName(draft.name)
      const agent = agentForProvider(draft.model.provider)
      // Checked by the flow before it asks; a draft that is not one is refused here too.
      if (name === null || agent === undefined) throw new ChannelCreationError("invalid")
      try {
        const client = await options.connected()
        const channel = await client.channels.create({
          requestId,
          name,
          ...(draft.folder === null ? {} : { folder: draft.folder }),
          agent,
          model: draft.model.modelId,
          approvalMode: draft.access,
          ...(draft.notes.trim() === "" ? {} : { notes: draft.notes }),
          ...(draft.resources.length === 0 ? {} : { resources: [...draft.resources] }),
        })
        return channelFrom(channel)
      } catch (error) {
        const code = channelRefusal(error)
        if (code !== undefined) throw new ChannelCreationError(failures[code])
        console.error("Making a channel got no answer the window knows", error)
        throw new ChannelCreationError("unavailable")
      }
    },
  }
}
