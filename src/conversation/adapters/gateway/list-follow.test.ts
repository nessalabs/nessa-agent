import {
  createSubscriptionGate,
  NessaRpcError,
  type ListSubscriptionHandlers,
  type NessaClient,
} from "@nessa/client"
import { expect, it, vi } from "vitest"
import { FOLLOW_RETRY_MS, gatewayEffects } from "./effects"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
const flush = async () => {
  for (let i = 0; i < 15; i++) await Promise.resolve()
}
function gateway(
  answers: Array<Promise<void> | Error> = [],
  closeWait: () => Promise<void> = async () => {},
) {
  const opens: Array<{
    archived: boolean
    handlers: ListSubscriptionHandlers
    close: ReturnType<typeof vi.fn>
  }> = []
  const request = vi.fn(
    async (handlers: ListSubscriptionHandlers, options: { archived?: boolean } = {}) => {
      const answer = answers.shift()
      if (answer instanceof Error) throw answer
      await answer
      const close = vi.fn(closeWait)
      opens.push({ archived: options.archived ?? false, handlers, close })
      return { id: String(opens.length), close }
    },
  )
  const gate = createSubscriptionGate()
  const client = {
    subscriptions: {
      list: (
        handlers: ListSubscriptionHandlers,
        options: { archived?: boolean; signal?: AbortSignal } = {},
      ) =>
        gate(`list:${options.archived ?? false}`, options.signal, () =>
          request(handlers, options),
        ),
    },
  } as unknown as NessaClient
  return { client, opens, request }
}
function follower() {
  return { list: vi.fn(), failed: vi.fn() }
}

it("PL8: independent active and archived targets use the existing client gate and lagging resumes immediately", async () => {
  const g = gateway()
  const wait = vi.fn(async () => {})
  const effects = gatewayEffects(() => g.client, wait)
  const a = follower(),
    b = follower()
  const stopA = effects.followList(false, a),
    stopB = effects.followList(true, b)
  await flush()
  expect(g.opens.map((x) => x.archived)).toEqual([false, true])
  const list = { conversations: [], complete: false }
  g.opens[0]!.handlers.list(list)
  expect(a.list).toHaveBeenCalledWith(list)
  expect(b.list).not.toHaveBeenCalled()
  g.opens[0]!.handlers.ended({ reason: "lagging" })
  await flush()
  expect(g.opens).toHaveLength(3)
  expect(g.opens[2]!.archived).toBe(false)
  expect(wait).not.toHaveBeenCalled()
  stopA()
  stopB()
  expect(g.opens[1]!.close).toHaveBeenCalledOnce()
  expect(g.opens[2]!.close).toHaveBeenCalledOnce()
})

it("PL4: an aborted pending list open closes before its replacement can subscribe", async () => {
  const ack = deferred<void>(),
    closed = deferred<void>()
  const g = gateway([ack.promise], () => closed.promise)
  const effects = gatewayEffects(
    () => g.client,
    async () => {},
  )
  const old = follower(),
    next = follower()
  const stopOld = effects.followList(false, old)
  await flush()
  stopOld()
  const stopNext = effects.followList(false, next)
  await flush()
  expect(g.request).toHaveBeenCalledOnce()
  ack.resolve()
  await flush()
  expect(g.opens[0]!.close).toHaveBeenCalledOnce()
  g.opens[0]!.handlers.list({ conversations: [], complete: true })
  g.opens[0]!.handlers.ended({ reason: "refused", code: "forbidden" })
  expect(old.list).not.toHaveBeenCalled()
  expect(old.failed).not.toHaveBeenCalled()
  expect(g.request).toHaveBeenCalledOnce()
  closed.resolve()
  await flush()
  expect(g.request).toHaveBeenCalledTimes(2)
  stopNext()
})

it("PL3, PL8: typed refusal retains its failure through backoff; no terminal callback applies during that wait", async () => {
  const waited = deferred<void>()
  const wait = vi.fn(() => waited.promise)
  const g = gateway()
  const effects = gatewayEffects(() => g.client, wait)
  const f = follower()
  const stop = effects.followList(true, f)
  await flush()
  g.opens[0]!.handlers.ended({ reason: "refused", code: "conversation_state_unreadable" })
  expect(f.failed).toHaveBeenCalledWith("state-unreadable", expect.anything())
  expect(wait).toHaveBeenCalledExactlyOnceWith(FOLLOW_RETRY_MS)
  g.opens[0]!.handlers.list({ conversations: [], complete: true })
  g.opens[0]!.handlers.ended({ reason: "lagging" })
  expect(f.list).not.toHaveBeenCalled()
  expect(g.request).toHaveBeenCalledOnce()
  waited.resolve()
  await flush()
  expect(g.opens).toHaveLength(2)
  stop()
  g.opens[1]!.handlers.list({ conversations: [], complete: true })
  expect(f.list).not.toHaveBeenCalled()
})

it("PL8: a refused open reports the port's typed reason and a stopped retry sends nothing", async () => {
  const waited = deferred<void>()
  const wait = vi.fn(() => waited.promise)
  const g = gateway([new NessaRpcError("conversation_configuration_changed", "changed")])
  const f = follower()
  const stop = gatewayEffects(() => g.client, wait).followList(false, f)
  await flush()
  expect(f.failed).toHaveBeenCalledWith("configuration-changed", expect.anything())
  stop()
  waited.resolve()
  await flush()
  expect(g.request).toHaveBeenCalledOnce()
})
