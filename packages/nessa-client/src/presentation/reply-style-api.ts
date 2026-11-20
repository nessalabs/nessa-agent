import type { RpcRequester } from "../application/session-port.js"
import {
  ProductMethod,
  type ReplyStyleChoice,
  type ReplyStyleRole,
  type ReplyStyleRoles,
  type ReplyStyles,
} from "../generated/product.js"

/**
 * How agents talk: the caller's Nessa-level choice for each role, and each
 * channel's own. A level with no choice follows the one above it, and above
 * the Nessa level is Nessa's own plain style.
 */
export type ReplyStyleApi = {
  /** The Nessa level's choices and, with `channelId`, that channel's own. */
  read(channelId?: string): Promise<ReplyStyles>
  /**
   * Save `role`'s style at the Nessa level or for `channelId`; `undefined`
   * removes this level's choice so it follows the level above. Reaches
   * conversations created after it: a conversation keeps the style it
   * started with. Not replayed: read again after an uncertain answer.
   */
  set(
    role: ReplyStyleRole,
    choice: ReplyStyleChoice | undefined,
    channelId?: string,
  ): Promise<ReplyStyles>
}

export function createReplyStyleApi(session: RpcRequester): ReplyStyleApi {
  return {
    read: async (channelId) =>
      replyStyles(
        await session.request(
          ProductMethod.ReplyStyleRead,
          channelId === undefined ? {} : { channelId },
        ),
      ),
    set: async (role, choice, channelId) =>
      replyStyles(
        await session.request(ProductMethod.ReplyStyleSet, {
          ...(channelId === undefined ? {} : { channelId }),
          role,
          ...(choice === undefined ? { mode: "default" } : choice),
        }),
      ),
  }
}

const MAX_TEXT_BYTES = 4096
const MAX_CHANNEL_ID_BYTES = 256

/** Reads a `replyStyle.read` or `replyStyle.set` answer, refusing anything off its schema. */
export function replyStyles(value: unknown): ReplyStyles {
  const invalid = () => new TypeError("Invalid reply styles")
  if (
    !isRecord(value) ||
    !onlyKeys(value, ["nessa", "effective"], ["channelId", "channel"]) ||
    (Object.hasOwn(value, "channelId") &&
      (typeof value.channelId !== "string" ||
        value.channelId.length === 0 ||
        bytes(value.channelId) > MAX_CHANNEL_ID_BYTES)) ||
    Object.hasOwn(value, "channelId") !== Object.hasOwn(value, "channel") ||
    !isRecord(value.effective) ||
    !onlyKeys(value.effective, ["coordinator", "thread"], [])
  )
    throw invalid()
  return {
    ...(Object.hasOwn(value, "channelId")
      ? { channelId: value.channelId as string }
      : {}),
    nessa: roles(value.nessa),
    ...(Object.hasOwn(value, "channel") ? { channel: roles(value.channel) } : {}),
    effective: {
      coordinator: choiceOf(value.effective.coordinator),
      thread: choiceOf(value.effective.thread),
    },
  }
}

function roles(value: unknown): ReplyStyleRoles {
  if (!isRecord(value) || !onlyKeys(value, [], ["coordinator", "thread"]))
    throw new TypeError("Invalid reply styles")
  return {
    ...(Object.hasOwn(value, "coordinator")
      ? { coordinator: choiceOf(value.coordinator) }
      : {}),
    ...(Object.hasOwn(value, "thread") ? { thread: choiceOf(value.thread) } : {}),
  }
}

function choiceOf(value: unknown): ReplyStyleChoice {
  const invalid = () => new TypeError("Invalid reply styles")
  if (!isRecord(value) || !onlyKeys(value, ["mode"], ["text"])) throw invalid()
  if (value.mode === "plain" || value.mode === "off") {
    if (Object.hasOwn(value, "text")) throw invalid()
    return { mode: value.mode }
  }
  if (
    value.mode !== "custom" ||
    typeof value.text !== "string" ||
    value.text.length === 0 ||
    bytes(value.text) > MAX_TEXT_BYTES
  )
    throw invalid()
  return { mode: "custom", text: value.text }
}

function bytes(text: string): number {
  return new TextEncoder().encode(text).length
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function onlyKeys(
  value: Record<string, unknown>,
  required: readonly string[],
  optional: readonly string[],
): boolean {
  return (
    Object.keys(value).every((key) => required.includes(key) || optional.includes(key)) &&
    required.every((key) => Object.hasOwn(value, key))
  )
}
