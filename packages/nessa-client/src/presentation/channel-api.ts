import { NessaRpcError } from "../application/rpc-error.js"
import type { RpcRequester } from "../application/session-port.js"
import { ChannelErrorCode, ProductMethod, channelRules } from "../generated/product.js"
import type {
  ApprovalMode,
  Channel,
  ChannelCreateParams,
  ChannelListResult,
  ChannelSetPlacementParams,
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
  /**
   * Every one of the caller's channels, the default one first. The gateway
   * answers a page per frame; this reads each page in turn, so `complete` is
   * always true on what it returns.
   */
  list(): Promise<ChannelListResult>
  /**
   * Change where a made channel's agents run. Threads already made keep
   * running where they started. Moving the main agent answers a fresh
   * `coordinatorConversationId`: the next main-agent chat starts there, and
   * the old one stays in the channel as an ordinary thread.
   */
  setPlacement(params: ChannelSetPlacementParams): Promise<Channel>
}

export function createChannelApi(session: RpcRequester): ChannelApi {
  return {
    create: async (params) => {
      const answer = await session.request(ProductMethod.ChannelCreate, params)
      if (!isRecord(answer) || !onlyKeys(answer, ["channel"]))
        throw new Error("Invalid channel create response")
      return channel(answer.channel, "Invalid channel create response")
    },
    setPlacement: async (params) => {
      const answer = await session.request(ProductMethod.ChannelSetPlacement, params)
      if (!isRecord(answer) || !onlyKeys(answer, ["channel"]))
        throw new Error("Invalid channel placement response")
      return channel(answer.channel, "Invalid channel placement response")
    },
    list: async () => {
      const channels: Channel[] = []
      let after: string | undefined
      // Each page past the first holds at least one channel the last did not,
      // so the walk ends within the most channels an owner may make.
      for (let page = 0; page <= MAX_CHANNEL_PAGES; page += 1) {
        const answer = await session.request(
          ProductMethod.ChannelList,
          after === undefined ? {} : { after },
        )
        const listed = listPage(answer)
        channels.push(...listed.channels)
        if (listed.complete) return { channels, complete: true }
        if (listed.cursor === undefined || listed.cursor === after)
          throw new Error("Invalid channel list response")
        after = listed.cursor
      }
      throw new Error("Invalid channel list response")
    },
  }
}

/** One stored channel per page at least, after the default one. */
const MAX_CHANNEL_PAGES = channelRules.maxChannels + 1

function listPage(answer: unknown): ChannelListResult {
  if (
    !isRecord(answer) ||
    !onlyKeys(
      answer,
      Object.hasOwn(answer, "cursor")
        ? ["channels", "complete", "cursor"]
        : ["channels", "complete"],
    ) ||
    !Array.isArray(answer.channels) ||
    typeof answer.complete !== "boolean" ||
    // A cursor is there exactly when the page is cut, and names a made channel.
    answer.complete === Object.hasOwn(answer, "cursor") ||
    (Object.hasOwn(answer, "cursor") &&
      (typeof answer.cursor !== "string" || !MADE_CHANNEL_ID.test(answer.cursor)))
  )
    throw new Error("Invalid channel list response")
  return {
    channels: answer.channels.map((value) =>
      channel(value, "Invalid channel list response"),
    ),
    complete: answer.complete,
    ...(typeof answer.cursor === "string" ? { cursor: answer.cursor } : {}),
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
  "coordinatorConversationId",
  "coordinatorPlacement",
  "threadPlacement",
] as const
const MODES = new Set<string>(["ask", "auto", "full"])
const CHANNEL_ID = /^(default|ch_[a-z2-7]{10})$/
const MADE_CHANNEL_ID = /^ch_[a-z2-7]{10}$/

function channel(value: unknown, invalid: string): Channel {
  if (!isRecord(value) || !onlyKeys(value, CHANNEL_KEYS)) throw new Error(invalid)
  const nullableText = (field: unknown) => field === null || typeof field === "string"
  if (
    !nullableText(value.coordinatorConversationId) ||
    (value.coordinatorPlacement !== "local" && value.coordinatorPlacement !== "boat") ||
    (value.threadPlacement !== "local" && value.threadPlacement !== "boat") ||
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
    coordinatorConversationId: value.coordinatorConversationId as string | null,
    coordinatorPlacement: value.coordinatorPlacement as "local" | "boat",
    threadPlacement: value.threadPlacement as "local" | "boat",
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function onlyKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return (
    Object.keys(value).every((key) => keys.includes(key)) &&
    keys.every((key) => Object.hasOwn(value, key))
  )
}
