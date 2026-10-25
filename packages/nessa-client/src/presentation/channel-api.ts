import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ChannelErrorCode, ProductMethod } from "../generated/product.js"
import type {
  ApprovalMode,
  Channel,
  ChannelCreateParams,
  ChannelListResult,
} from "../generated/product.js"

/**
 * The person's channels: named places their conversations live in. A channel
 * holds what a new conversation in it starts with (agent, model, approval
 * preset), the folder its agents work in, and notes and files every agent in
 * it is told.
 *
 * `create` is the one call that names a folder. A conversation names its
 * channel by `channelId` (`conversation.create`), never a folder.
 */
export type ChannelApi = {
  /**
   * Make a channel. Without `folder`, the gateway makes one under its own
   * channels folder. Sending the same `requestId` again answers the channel
   * it made; a different channel under it is refused `channel_request_conflict`.
   */
  create(params: ChannelCreateParams): Promise<Channel>
  /** The caller's channels, the default one first. */
  list(): Promise<ChannelListResult>
}

export function createChannelApi(session: RpcRequester): ChannelApi {
  return {
    create: async (params) => {
      const answer = await session.request(ProductMethod.ChannelCreate, params)
      if (!isRecord(answer) || !onlyKeys(answer, ["channel"]))
        throw new Error("Invalid channel create response")
      return channel(answer.channel, "Invalid channel create response")
    },
    list: async () => {
      const answer = await session.request(ProductMethod.ChannelList, {})
      if (
        !isRecord(answer) ||
        !onlyKeys(answer, ["channels", "complete"]) ||
        !Array.isArray(answer.channels) ||
        typeof answer.complete !== "boolean"
      )
        throw new Error("Invalid channel list response")
      return {
        channels: answer.channels.map((value) =>
          channel(value, "Invalid channel list response"),
        ),
        complete: answer.complete,
      }
    },
  }
}

const refusals = new Set<string>(Object.values(ChannelErrorCode))

/** The typed reason a channel call was refused, when it was one this build knows. */
export function channelRefusal(error: unknown): ChannelErrorCode | undefined {
  return error instanceof NessaRpcError && refusals.has(error.code)
    ? (error.code as ChannelErrorCode)
    : undefined
}

const CHANNEL_KEYS = [
  "channelId",
  "name",
  "folder",
  "folderMade",
  "agent",
  "model",
  "approvalMode",
  "createdAtMs",
] as const
const MODES = new Set<string>(["ask", "auto", "full"])
const CHANNEL_ID = /^(default|ch_[a-z2-7]{10})$/

function channel(value: unknown, invalid: string): Channel {
  if (!isRecord(value) || !onlyKeys(value, CHANNEL_KEYS)) throw new Error(invalid)
  const nullableText = (field: unknown) => field === null || typeof field === "string"
  if (
    typeof value.channelId !== "string" ||
    !CHANNEL_ID.test(value.channelId) ||
    typeof value.name !== "string" ||
    typeof value.folder !== "string" ||
    typeof value.folderMade !== "boolean" ||
    !nullableText(value.agent) ||
    !nullableText(value.model) ||
    typeof value.approvalMode !== "string" ||
    !MODES.has(value.approvalMode) ||
    !Number.isSafeInteger(value.createdAtMs) ||
    (value.createdAtMs as number) < 0
  )
    throw new Error(invalid)
  return {
    channelId: value.channelId,
    name: value.name,
    folder: value.folder,
    folderMade: value.folderMade,
    agent: value.agent as string | null,
    model: value.model as string | null,
    approvalMode: value.approvalMode as ApprovalMode,
    createdAtMs: value.createdAtMs as number,
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function onlyKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return (
    Object.keys(value).every((key) => keys.includes(key)) &&
    keys.every((key) => key in value)
  )
}
