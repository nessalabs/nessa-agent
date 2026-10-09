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
import { createSubscriptionGate } from "./subscription-gate.js"

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
 * again — a view from the last cursor it applied.
 *
 * `signal` gives an open up: aborted before the answer, the open rejects with
 * the signal's reason and the subscription is closed once answered; aborted
 * after, it is closed. Either way the next open of the same target is sent
 * only after that close (`subscription-gate.ts`), so a follower that stops and
 * follows again is never refused as its own duplicate. */
export interface SubscriptionApi {
  /** Follow one conversation's view. With `after`, nothing behind that cursor is sent. */
  view(
    conversationId: string,
    handlers: ViewSubscriptionHandlers,
    options?: { after?: ConversationViewCursor; signal?: AbortSignal },
  ): Promise<Subscription>
  /** Follow the caller's conversation list. */
  list(
    handlers: ListSubscriptionHandlers,
    options?: { archived?: boolean; signal?: AbortSignal },
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

/** What arrived for an identity before its open registered it. */
type Unclaimed = { frame?: { event: string; payload: unknown }; end?: SubscriptionEnd }

export function createSubscriptionApi(session: SubscriptionPort): SubscriptionApi {
  // The first read can take the passive delivery budget before the reply.
  const deadline = { atLeastMs: passiveReadTiming.minRequestTimeoutMs }
  const live = new Map<string, Live>()
  const gated = createSubscriptionGate()
  // Changes whenever the connection is lost: an identity from before means nothing after.
  let connection = 0
  // Frames of no live subscription, kept while an open is on its way: the
  // transport may hand over a subscribe's answer and its first frames in one
  // turn, before the open registers the identity the answer names. Its open
  // claims them; nothing else does. A frame is a replacement, so the last one
  // of each identity is enough, and its end after it. A late frame of a
  // subscription already closed is kept too, never claimed (the gateway
  // never reuses an identity on a connection: `minted` in
  // `crates/nessa-server/src/product/subscription/connection.rs`): at most one frame for each
  // identity, and all of them let go once no open is on its way.
  let opening = 0
  const unclaimed = new Map<string, Unclaimed>()
  const hold = (id: string) => {
    if (opening === 0) return undefined
    const held = unclaimed.get(id) ?? {}
    unclaimed.set(id, held)
    return held
  }

  const finish = (id: string, end: SubscriptionEnd) => {
    const entry = live.get(id)
    if (!entry) {
      const held = hold(id)
      if (held) held.end ??= end
      return
    }
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
    if (id === undefined) return
    const entry = live.get(id)
    if (!entry) {
      const held = hold(id)
      if (held && held.end === undefined) held.frame = { event, payload }
      return
    }
    if (entry.event !== event) return
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
    unclaimed.clear()
    for (const id of [...live.keys()]) finish(id, { reason: "disconnected" })
  })

  async function open(
    method: string,
    params: unknown,
    entry: Live,
    signal: AbortSignal | undefined,
  ): Promise<Subscription> {
    const on = connection
    let subscriptionId: string
    let held: Unclaimed | undefined
    opening += 1
    try {
      ;({ subscriptionId } = subscribeResult(
        await session.request(method, params, deadline),
      ))
      held = unclaimed.get(subscriptionId)
      unclaimed.delete(subscriptionId)
    } finally {
      opening -= 1
      if (opening === 0) unclaimed.clear()
    }
    if (on !== connection) {
      // Answered just before that connection was lost.
      queueMicrotask(() => entry.ended({ reason: "disconnected" }))
      return { id: subscriptionId, close: async () => undefined }
    }
    live.set(subscriptionId, entry)
    // What arrived with the answer, in the order it came; nothing of an open
    // given up on its way, which the gate closes next.
    if (signal?.aborted) held = undefined
    if (held?.frame) route(held.frame.event, held.frame.payload)
    // A held frame that failed its checks has ended it already.
    if (held?.end && live.has(subscriptionId)) finish(subscriptionId, held.end)
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
      return gated(`view:${conversationId}`, options.signal, () =>
        open(
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
          options.signal,
        ),
      )
    },
    list: (handlers, options = {}) => {
      const archived = options.archived ?? false
      return gated(`list:${archived}`, options.signal, () =>
        open(
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
          options.signal,
        ),
      )
    },
  }
}
