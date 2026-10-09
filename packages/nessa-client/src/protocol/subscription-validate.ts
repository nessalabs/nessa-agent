import {
  conversationSubscriptionIdPattern,
  ConversationSubscriptionEndReason,
  maxViewCursorIncarnationLength,
} from "../generated/product.js"
import type {
  ConversationListResult,
  ConversationSubscribeResult,
  ConversationSubscriptionEnded,
  ConversationView,
  ConversationViewCursor,
} from "../generated/product.js"
import { conversationList, conversationView } from "./conversation-validate.js"
import { decimal } from "./passive-read-validate.js"

const ID = new RegExp(conversationSubscriptionIdPattern)
const REASONS: readonly string[] = Object.values(ConversationSubscriptionEndReason)

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function only(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return Object.keys(value).every((key) => keys.includes(key))
}

/** An identity this connection minted; it means nothing on another one. */
export function validSubscriptionId(value: unknown): value is string {
  return typeof value === "string" && ID.test(value)
}

/** A view cursor as the schema bounds it. */
export function validViewCursor(value: unknown): value is ConversationViewCursor {
  return (
    record(value) &&
    only(value, ["incarnation", "position"]) &&
    typeof value.incarnation === "string" &&
    value.incarnation.length > 0 &&
    [...value.incarnation].length <= maxViewCursorIncarnationLength &&
    decimal(value.position)
  )
}

export function subscribeResult(value: unknown): ConversationSubscribeResult {
  if (!record(value) || !only(value, ["subscriptionId"]))
    throw new TypeError("Invalid subscription response")
  if (!validSubscriptionId(value.subscriptionId))
    throw new TypeError("Invalid subscription response")
  return { subscriptionId: value.subscriptionId }
}

/** The subscription a frame names, before its body is checked by the one that holds it. */
export function framedSubscription(value: unknown): string | undefined {
  return record(value) && validSubscriptionId(value.subscriptionId)
    ? value.subscriptionId
    : undefined
}

/** A `conversation.view` frame for `conversationId`, checked as `conversation.read` is. */
export function viewedFrame(
  value: unknown,
  conversationId: string,
): { cursor: ConversationViewCursor; view: ConversationView } {
  if (!record(value) || !only(value, ["subscriptionId", "cursor", "view"]))
    throw new TypeError("Invalid conversation view frame")
  if (!validViewCursor(value.cursor))
    throw new TypeError("Invalid conversation view cursor")
  return { cursor: value.cursor, view: conversationView(value.view, conversationId) }
}

/** A `conversation.listed` frame, checked as `conversation.list` is. */
export function listedFrame(value: unknown, archived: boolean): ConversationListResult {
  if (!record(value) || !only(value, ["subscriptionId", "list"]))
    throw new TypeError("Invalid conversation list frame")
  return conversationList(value.list, archived)
}

/** A `conversation.subscriptionEnded` frame. */
export function endedFrame(value: unknown): ConversationSubscriptionEnded {
  if (
    !record(value) ||
    !only(value, ["subscriptionId", "reason", "code", "lastDelivered"]) ||
    !validSubscriptionId(value.subscriptionId) ||
    typeof value.reason !== "string" ||
    !REASONS.includes(value.reason) ||
    (Object.hasOwn(value, "code") &&
      (typeof value.code !== "string" || value.code.length === 0)) ||
    (Object.hasOwn(value, "lastDelivered") && !validViewCursor(value.lastDelivered))
  )
    throw new TypeError("Invalid subscription end frame")
  return value as unknown as ConversationSubscriptionEnded
}
