import {
  conversationSubscriptionIdPattern,
  ConversationSubscriptionEndReason,
  maxViewCursorIncarnationLength,
  ConversationEventKind,
  eventSubscriptionBounds,
} from "../generated/product.js"
import type {
  AgentsListResult,
  ConversationListResult,
  ConversationSubscribeResult,
  ConversationSubscriptionEnded,
  ConversationView,
  ConversationViewCursor,
  ConversationEvent,
  ConversationEvents,
  ConversationSubscribeEventsParams,
} from "../generated/product.js"
import { agentsList } from "./agents-validate.js"
import { conversationList, conversationView } from "./conversation-validate.js"
import { decimal } from "./passive-read-validate.js"

const ID = new RegExp(conversationSubscriptionIdPattern)
const REASONS: readonly string[] = Object.values(ConversationSubscriptionEndReason)
const EVENT_KINDS: readonly string[] = Object.values(ConversationEventKind)

function boundedCharacters(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length > 0 && [...value].length <= max
}

/** Opaque event cursors are bounded at the boundary and interpreted by the gateway. */
export function validEventCursor(value: unknown): value is string {
  return boundedCharacters(value, eventSubscriptionBounds.maxCursorCharacters)
}

/** Capture and validate event subscription input before the first await. */
export function eventSubscribeOptions(
  conversationId: string,
  options: { after?: string; replay?: boolean; kinds?: readonly ConversationEventKind[] },
): ConversationSubscribeEventsParams {
  const { after, replay } = options
  if (after !== undefined && !validEventCursor(after))
    throw new TypeError("Invalid event cursor")
  if (replay !== undefined && typeof replay !== "boolean")
    throw new TypeError("Invalid replay option")
  if (after !== undefined && replay !== undefined)
    throw new TypeError("Choose after or replay")
  if (
    options.kinds !== undefined &&
    (!Array.isArray(options.kinds) ||
      options.kinds.length === 0 ||
      options.kinds.length > EVENT_KINDS.length)
  )
    throw new TypeError("Invalid event kinds")
  const kinds = options.kinds === undefined ? undefined : [...options.kinds]
  if (
    kinds !== undefined &&
    (kinds.length === 0 ||
      kinds.length > EVENT_KINDS.length ||
      new Set(kinds).size !== kinds.length ||
      kinds.some((kind) => !EVENT_KINDS.includes(kind)))
  )
    throw new TypeError("Invalid event kinds")
  return {
    conversationId,
    ...(after === undefined ? {} : { after }),
    ...(replay === undefined ? {} : { replay }),
    ...(kinds === undefined ? {} : { kinds }),
  }
}

/** A bounded semantic batch. Its cursor is a checkpoint, not a view cursor. */
export function eventsFrame(value: unknown): Omit<ConversationEvents, "subscriptionId"> {
  if (
    !record(value) ||
    !only(value, ["subscriptionId", "events", "cursor"]) ||
    !validEventCursor(value.cursor) ||
    !Array.isArray(value.events) ||
    value.events.length > eventSubscriptionBounds.maxBatchEvents
  )
    throw new TypeError("Invalid event batch")
  const events = value.events.map((event: unknown): ConversationEvent => {
    if (
      !record(event) ||
      !only(event, [
        "id",
        "cursor",
        "kind",
        "executionId",
        "toolId",
        "name",
        "status",
        "outcome",
      ]) ||
      !validEventCursor(event.id) ||
      !validEventCursor(event.cursor) ||
      typeof event.kind !== "string" ||
      !EVENT_KINDS.includes(event.kind)
    )
      throw new TypeError("Invalid semantic event")
    for (const [key, max] of [
      ["executionId", eventSubscriptionBounds.maxExecutionIdCharacters],
      ["toolId", eventSubscriptionBounds.maxToolIdCharacters],
      ["name", eventSubscriptionBounds.maxNameCharacters],
      ["status", eventSubscriptionBounds.maxStatusCharacters],
      ["outcome", eventSubscriptionBounds.maxOutcomeCharacters],
    ] as const) {
      if (Object.hasOwn(event, key) && !boundedCharacters(event[key], max))
        throw new TypeError("Invalid semantic event metadata")
    }
    return { ...event } as unknown as ConversationEvent
  })
  return { cursor: value.cursor, events }
}

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

/** An `agents.listed` frame, checked as `agents.list` is. */
export function agentsListedFrame(value: unknown): AgentsListResult {
  if (!record(value) || !only(value, ["subscriptionId", "catalog"]))
    throw new TypeError("Invalid agent catalog frame")
  return agentsList(value.catalog)
}

/** A `conversation.subscriptionEnded` frame. */
export function endedFrame(value: unknown): ConversationSubscriptionEnded {
  if (
    !record(value) ||
    !only(value, [
      "subscriptionId",
      "reason",
      "code",
      "lastDelivered",
      "lastEventCursor",
    ]) ||
    !validSubscriptionId(value.subscriptionId) ||
    typeof value.reason !== "string" ||
    !REASONS.includes(value.reason) ||
    (Object.hasOwn(value, "code") &&
      (typeof value.code !== "string" || value.code.length === 0)) ||
    (Object.hasOwn(value, "lastDelivered") && !validViewCursor(value.lastDelivered)) ||
    (Object.hasOwn(value, "lastEventCursor") && !validEventCursor(value.lastEventCursor))
  )
    throw new TypeError("Invalid subscription end frame")
  return value as unknown as ConversationSubscriptionEnded
}
