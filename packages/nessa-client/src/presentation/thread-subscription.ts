import type { ConversationEvents, ConversationEventKind } from "../generated/product.js"
import type {
  Subscription,
  SubscriptionApi,
  SubscriptionEnd,
} from "./subscription-api.js"
import { eventSubscriptionBufferBatches } from "./subscription-api.js"

/** One ordered batch of events; retain cursor after applying the whole batch. */
export type ThreadEventBatch = Omit<ConversationEvents, "subscriptionId">

/** Resume position, event filter, and optional lifetime of an event subscription. */
export type ThreadSubscribeOptions = {
  /** Opaque cursor of the last batch the caller applied. */
  after?: string
  /** Start at the beginning instead of the default captured head; cannot accompany after. */
  replay?: boolean
  /** Omitted follows every event kind. */
  kinds?: readonly ConversationEventKind[]
  /** Aborting closes the subscription and completes a pending next(). */
  signal?: AbortSignal
}

/** A single-consumer event stream. A for-await break releases it through return(). */
export interface ThreadSubscription extends AsyncIterable<ThreadEventBatch> {
  /** Release the gateway subscription and buffered batches; repeated calls share cleanup. */
  close(): Promise<void>
}

/** An event stream ended before the caller closed it. Resume from your last applied cursor. */
export class NessaThreadSubscriptionError extends Error {
  constructor(
    /** Gateway or client termination evidence. lastEventCursor describes wire delivery, not application consumption. */
    readonly end: SubscriptionEnd,
  ) {
    super(`Thread subscription ended: ${end.reason}`)
    this.name = "NessaThreadSubscriptionError"
  }
}

/** Adapt the existing validated subscription lifecycle into a bounded async iterator. */
export async function openThreadSubscription(
  subscriptions: Pick<SubscriptionApi, "events">,
  threadId: string,
  options: ThreadSubscribeOptions = {},
): Promise<ThreadSubscription> {
  const signal = options.signal
  const queue: ThreadEventBatch[] = []
  let waiting:
    | {
        resolve(value: IteratorResult<ThreadEventBatch>): void
        reject(error: unknown): void
      }
    | undefined
  let failure: unknown
  let failed = false
  let done = false
  let opened: Subscription | undefined
  let closing: Promise<void> | undefined
  const removeAbort = () => signal?.removeEventListener("abort", abort)
  const release = () => {
    if (!opened) return Promise.resolve()
    closing ??= opened.close()
    return closing
  }
  const finish = (error?: unknown, discard = false) => {
    if (done) return
    done = true
    if (discard) queue.length = 0
    removeAbort()
    const pending = waiting
    waiting = undefined
    if (error !== undefined) {
      if (pending) pending.reject(error)
      else {
        failure = error
        failed = true
      }
    } else pending?.resolve({ done: true, value: undefined })
  }
  const abort = () => {
    finish(undefined, true)
    void release().catch(() => undefined)
  }
  const close = () => {
    finish(undefined, true)
    queue.length = 0
    failed = false
    failure = undefined
    return release()
  }
  try {
    opened = await subscriptions.events(
      threadId,
      {
        batch: (batch) => {
          if (done) return
          if (waiting) {
            const pending = waiting
            waiting = undefined
            pending.resolve({ done: false, value: batch })
          } else if (queue.length < eventSubscriptionBufferBatches) queue.push(batch)
          else {
            finish(new NessaThreadSubscriptionError({ reason: "lagging" }), true)
            void release().catch(() => undefined)
          }
        },
        ended: (end) => {
          finish(new NessaThreadSubscriptionError(end))
          void release().catch(() => undefined)
        },
      },
      options,
    )
  } catch (error) {
    finish()
    throw error
  }
  // Frames, termination, or abort may arrive before the open's answer.
  if (done || signal?.aborted) {
    if (signal?.aborted) finish(undefined, true)
    await release()
  } else signal?.addEventListener("abort", abort, { once: true })

  const iterator: AsyncIterator<ThreadEventBatch> = {
    next: () => {
      if (waiting)
        return Promise.reject(
          new TypeError("Await the previous next() before calling next() again"),
        )
      const value = queue.shift()
      if (value) return Promise.resolve({ done: false, value })
      if (failed) {
        failed = false
        const error = failure
        failure = undefined
        return Promise.reject(error)
      }
      if (done) return Promise.resolve({ done: true, value: undefined })
      return new Promise((resolve, reject) => {
        waiting = { resolve, reject }
      })
    },
    return: async () => {
      await close()
      return { done: true, value: undefined }
    },
    throw: async (error?: unknown) => {
      await close()
      throw error
    },
  }
  let claimed = false
  return {
    close,
    [Symbol.asyncIterator]: () => {
      if (claimed) throw new TypeError("A thread subscription has one consumer")
      claimed = true
      return iterator
    },
  }
}
