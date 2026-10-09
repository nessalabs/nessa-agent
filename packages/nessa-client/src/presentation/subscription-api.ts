import type { RpcRequester } from "../application/session-port.js"
import type { ConnectionState } from "../application/managed-session.js"
import { passiveReadTiming, ProductEvent, ProductMethod } from "../generated/product.js"
import type {
  ConversationListResult,
  ConversationSubscriptionEnded,
  ConversationView,
  ConversationViewCursor,
} from "../generated/product.js"
import { conversationIdPattern } from "../protocol/conversation-validate.js"
import {
  endedFrame,
  framedSubscription,
  listedFrame,
  subscribeResult,
  validViewCursor,
  viewedFrame,
} from "../protocol/subscription-validate.js"

/** Why a subscription ended. The gateway's reasons, and two of this client's:
 * `disconnected` when the connection it lived on was lost (its identity goes
 * with it; subscribe again on the next one), and `invalid_frame` when a frame
 * failed the same checks a one-shot read's answer must pass. */
export type SubscriptionEnd = Omit<
  ConversationSubscriptionEnded,
  "subscriptionId" | "reason"
> & {
  reason: ConversationSubscriptionEnded["reason"] | "disconnected" | "invalid_frame"
}

/** One view frame: the bounded replacement `conversation.read` returns, and where it was read. */
export type ViewFrame = { cursor: ConversationViewCursor; view: ConversationView }

export type ViewSubscriptionHandlers = {
  /** Replace what is held with this view. */
  view(frame: ViewFrame): void
  /** Called once; nothing of the subscription follows. */
  ended(end: SubscriptionEnd): void
}

export type ListSubscriptionHandlers = {
  /** Replace what is held with this list. */
  list(list: ConversationListResult): void
  /** Called once; nothing of the subscription follows. */
  ended(end: SubscriptionEnd): void
}

/** A live subscription on this connection. */
export interface Subscription {
  readonly id: string
  /** Stop it. Nothing of it is delivered once this is called. */
  close(): Promise<void>
}

/** Replay-to-live views and lists. Each lives on the connection it was made on;
 * when that connection is lost it ends `disconnected`, and the caller subscribes
 * again — a view from the last cursor it applied. */
export interface SubscriptionApi {
  /** Follow one conversation's view. With `after`, nothing behind that cursor is sent. */
  view(
    conversationId: string,
    handlers: ViewSubscriptionHandlers,
    options?: { after?: ConversationViewCursor },
  ): Promise<Subscription>
  /** Follow the caller's conversation list. */
  list(
    handlers: ListSubscriptionHandlers,
    options?: { archived?: boolean },
  ): Promise<Subscription>
}

/** What the API needs of the managed session: requests, events, and its state. */
export interface SubscriptionPort extends RpcRequester {
  onEvent(event: string, handler: (payload: unknown) => void): () => void
  onState(handler: (state: ConnectionState) => void): () => void
}

type Live = {
  event: string
  /** Check a frame; the returned delivery hands it to the caller. */
  check(payload: unknown): () => void
  ended(end: SubscriptionEnd): void
}

export function createSubscriptionApi(session: SubscriptionPort): SubscriptionApi {
  // The first read can take the passive delivery budget before the reply.
  const deadline = { atLeastMs: passiveReadTiming.minRequestTimeoutMs }
  const live = new Map<string, Live>()
  // Changes whenever the connection is lost: an identity from before means nothing after.
  let connection = 0

  const finish = (id: string, end: SubscriptionEnd) => {
    const entry = live.get(id)
    if (!entry) return
    live.delete(id)
    entry.ended(end)
  }
  const unsubscribe = (id: string) =>
    session.request(ProductMethod.ConversationUnsubscribe, { subscriptionId: id }).then(
      () => undefined,
      // Already ended, or the connection is gone with it.
      () => undefined,
    )
  const route = (event: string, payload: unknown) => {
    const id = framedSubscription(payload)
    const entry = id === undefined ? undefined : live.get(id)
    if (id === undefined || !entry || entry.event !== event) return
    let deliver: () => void
    try {
      deliver = entry.check(payload)
    } catch {
      live.delete(id)
      void unsubscribe(id)
      entry.ended({ reason: "invalid_frame" })
      return
    }
    deliver()
  }
  session.onEvent(ProductEvent.ConversationView, (payload) =>
    route(ProductEvent.ConversationView, payload),
  )
  session.onEvent(ProductEvent.ConversationListed, (payload) =>
    route(ProductEvent.ConversationListed, payload),
  )
  session.onEvent(ProductEvent.ConversationSubscriptionEnded, (payload) => {
    let ended: ConversationSubscriptionEnded
    try {
      ended = endedFrame(payload)
    } catch {
      return
    }
    const { subscriptionId, ...end } = ended
    finish(subscriptionId, end)
  })
  session.onState((state) => {
    if (state.status === "connected") return
    connection += 1
    for (const id of [...live.keys()]) finish(id, { reason: "disconnected" })
  })

  async function open(
    method: string,
    params: unknown,
    entry: Live,
  ): Promise<Subscription> {
    const on = connection
    const { subscriptionId } = subscribeResult(
      await session.request(method, params, deadline),
    )
    if (on !== connection) {
      // Answered just before that connection was lost.
      queueMicrotask(() => entry.ended({ reason: "disconnected" }))
      return { id: subscriptionId, close: async () => undefined }
    }
    live.set(subscriptionId, entry)
    return {
      id: subscriptionId,
      close: async () => {
        if (live.get(subscriptionId) !== entry) return
        live.delete(subscriptionId)
        await unsubscribe(subscriptionId)
      },
    }
  }

  return {
    view: (conversationId, handlers, options = {}) => {
      if (!conversationIdPattern.test(conversationId))
        return Promise.reject(new TypeError("Invalid conversation ID"))
      if (options.after !== undefined && !validViewCursor(options.after))
        return Promise.reject(new TypeError("Invalid view cursor"))
      return open(
        ProductMethod.ConversationSubscribe,
        options.after === undefined
          ? { conversationId }
          : { conversationId, after: options.after },
        {
          event: ProductEvent.ConversationView,
          check: (payload) => {
            const frame = viewedFrame(payload, conversationId)
            return () => handlers.view(frame)
          },
          ended: (end) => handlers.ended(end),
        },
      )
    },
    list: (handlers, options = {}) => {
      const archived = options.archived ?? false
      return open(
        ProductMethod.ConversationSubscribeList,
        options.archived === undefined ? {} : { archived },
        {
          event: ProductEvent.ConversationListed,
          check: (payload) => {
            const list = listedFrame(payload, archived)
            return () => handlers.list(list)
          },
          ended: (end) => handlers.ended(end),
        },
      )
    },
  }
}
