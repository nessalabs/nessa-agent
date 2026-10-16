import { describe, expect, it, vi } from "vitest"
import type { EventSubscriptionHandlers, SubscriptionApi } from "./subscription-api.js"
import {
  openThreadSubscription,
  NessaThreadSubscriptionError,
  type ThreadEventBatch,
} from "./thread-subscription.js"

const id = "00000000-0000-4000-8000-000000000001"
const batch = (cursor: string): ThreadEventBatch => ({
  cursor,
  events: [{ id: cursor, cursor, kind: "state.changed", status: "opened" }],
})

function source(early?: (handlers: EventSubscriptionHandlers) => void) {
  let handlers!: EventSubscriptionHandlers
  const close = vi.fn(async () => undefined)
  const events: SubscriptionApi["events"] = vi.fn(async (_id, received) => {
    handlers = received
    early?.(received)
    return { id: "1", close }
  })
  return { api: { events }, close, handlers: () => handlers }
}

describe("thread event iteration", () => {
  it("retains early batches in order, including an empty filtered checkpoint", async () => {
    const wire = source((handlers) => {
      handlers.batch(batch("one"))
      handlers.batch({ cursor: "two", events: [] })
    })
    const subscription = await openThreadSubscription(wire.api, id)
    const iterator = subscription[Symbol.asyncIterator]()
    expect(await iterator.next()).toEqual({ done: false, value: batch("one") })
    expect(await iterator.next()).toEqual({
      done: false,
      value: { cursor: "two", events: [] },
    })
    const pending = iterator.next()
    await subscription.close()
    expect(await pending).toEqual({ done: true, value: undefined })
    await subscription.close()
    expect(wire.close).toHaveBeenCalledTimes(1)
  })

  it("fails and closes when a slow consumer exceeds bounded retention", async () => {
    const wire = source()
    const subscription = await openThreadSubscription(wire.api, id)
    for (let count = 0; count < 100; count++) wire.handlers().batch(batch(String(count)))
    const iterator = subscription[Symbol.asyncIterator]()
    await expect(iterator.next()).rejects.toMatchObject({ end: { reason: "lagging" } })
    expect(wire.close).toHaveBeenCalledTimes(1)
    expect(await iterator.next()).toEqual({ done: true, value: undefined })
  })

  it("drains delivered batches before exposing remote termination evidence", async () => {
    const wire = source()
    const subscription = await openThreadSubscription(wire.api, id)
    const iterator = subscription[Symbol.asyncIterator]()
    wire.handlers().batch(batch("one"))
    wire
      .handlers()
      .ended({ reason: "refused", code: "forbidden", lastEventCursor: "wire-only" })
    expect((await iterator.next()).value).toEqual(batch("one"))
    await expect(iterator.next()).rejects.toMatchObject({
      end: { reason: "refused", lastEventCursor: "wire-only" },
    })
  })

  it("for-await break releases the subscription", async () => {
    const wire = source((handlers) => handlers.batch(batch("one")))
    const subscription = await openThreadSubscription(wire.api, id)
    for await (const received of subscription) {
      expect(received.cursor).toBe("one")
      break
    }
    expect(wire.close).toHaveBeenCalledTimes(1)
    expect(() => subscription[Symbol.asyncIterator]()).toThrow(TypeError)
  })

  it("abort completes an outstanding next and releases it", async () => {
    const wire = source()
    const controller = new AbortController()
    const subscription = await openThreadSubscription(wire.api, id, {
      signal: controller.signal,
    })
    const pending = subscription[Symbol.asyncIterator]().next()
    controller.abort()
    expect(await pending).toEqual({ done: true, value: undefined })
    expect(wire.close).toHaveBeenCalledTimes(1)
  })

  it("handles termination or overflow before open resolves", async () => {
    const wire = source((handlers) => {
      for (let count = 0; count < 100; count++) handlers.batch(batch(String(count)))
    })
    const subscription = await openThreadSubscription(wire.api, id)
    await expect(subscription[Symbol.asyncIterator]().next()).rejects.toBeInstanceOf(
      NessaThreadSubscriptionError,
    )
    expect(wire.close).toHaveBeenCalledTimes(1)
  })

  it("refuses concurrent next calls without abandoning the first wait", async () => {
    const wire = source()
    const subscription = await openThreadSubscription(wire.api, id)
    const iterator = subscription[Symbol.asyncIterator]()
    const first = iterator.next()
    await expect(iterator.next()).rejects.toBeInstanceOf(TypeError)
    wire.handlers().batch(batch("one"))
    expect((await first).value).toEqual(batch("one"))
    await subscription.close()
  })
})
