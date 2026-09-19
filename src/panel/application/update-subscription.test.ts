import { describe, expect, it, vi } from "vitest"
import { attachThenAsk } from "./update-subscription"

function deferred<T>() {
  let settle!: (value: T) => void
  let fail!: (reason: unknown) => void
  const promise = new Promise<T>((done, broken) => {
    settle = done
    fail = broken
  })
  return { promise, settle, fail }
}

/** Let every already-settled promise run its continuations. */
const settled = async () => {
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve()
}

describe("attachThenAsk", () => {
  /**
   * The fault this exists to prevent: the host announced an update while the
   * listeners were still being attached, the question had already been asked
   * and answered "nothing", and the panel never heard about it again.
   */
  it("asks only once every listener is really attached", async () => {
    const attaching = deferred<() => void>()
    const asked = vi.fn(() => Promise.resolve())

    attachThenAsk({
      listeners: [() => Promise.resolve(vi.fn()), () => attaching.promise],
      ask: asked,
    })
    await settled()
    expect(asked).not.toHaveBeenCalled()

    attaching.settle(vi.fn())
    await settled()
    expect(asked).toHaveBeenCalledOnce()
  })

  /**
   * And `ready` waits for all of them, because a listener is live when its own
   * promise resolves: the available-update one can be attached and announcing
   * while the failure one is still pending, and an install started in that gap
   * is refused to nobody.
   */
  it("says the host can be heard from only when every listener can hear it", async () => {
    const attaching = deferred<() => void>()
    const order: string[] = []

    attachThenAsk({
      listeners: [() => Promise.resolve(vi.fn()), () => attaching.promise],
      ask: async () => {
        order.push("ask")
      },
      ready: () => order.push("ready"),
    })
    await settled()
    expect(order).toEqual([])

    attaching.settle(vi.fn())
    await settled()
    expect(order).toEqual(["ready", "ask"])
  })

  /**
   * One listener rejecting must not leave the others attached with their
   * handles thrown away — `Promise.all` discards the resolved values, so they
   * would go on firing into a panel that was never told it was listening.
   */
  it("takes down the listeners that did attach when one of them fails", async () => {
    const first = vi.fn()
    const second = vi.fn()
    const failing = deferred<() => void>()
    const failed = vi.fn()
    const ready = vi.fn()
    const asked = vi.fn(() => Promise.resolve())

    attachThenAsk({
      listeners: [
        () => Promise.resolve(first),
        () => Promise.resolve(second),
        () => failing.promise,
      ],
      ask: asked,
      ready,
      failed,
    })
    await settled()
    failing.fail(new Error("listen() failed"))
    await settled()

    expect(failed).toHaveBeenCalledOnce()
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledOnce()
    expect(ready).not.toHaveBeenCalled()
    expect(asked).not.toHaveBeenCalled()
  })

  /** Unmounted while registering: the listeners arrive with nobody wanting them. */
  it("takes down listeners that resolve after it was cancelled", async () => {
    const attaching = deferred<() => void>()
    const unlisten = vi.fn()
    const asked = vi.fn(() => Promise.resolve())

    const cancel = attachThenAsk({
      listeners: [() => attaching.promise],
      ask: asked,
    })
    cancel()
    attaching.settle(unlisten)
    await settled()

    expect(unlisten).toHaveBeenCalledOnce()
    expect(asked).not.toHaveBeenCalled()
  })

  /** And cancelling while one is still pending releases the ones already held. */
  it("releases what it holds when cancelled mid-registration", async () => {
    const held = vi.fn()
    const pending = deferred<() => void>()
    const late = vi.fn()

    const cancel = attachThenAsk({
      listeners: [() => Promise.resolve(held), () => pending.promise],
      ask: () => Promise.resolve(),
    })
    await settled()
    cancel()
    expect(held).toHaveBeenCalledOnce()

    pending.settle(late)
    await settled()
    expect(late).toHaveBeenCalledOnce()
  })

  it("takes down listeners it is holding when cancelled, and only once", async () => {
    const unlisten = vi.fn()
    const cancel = attachThenAsk({
      listeners: [() => Promise.resolve(unlisten)],
      ask: () => Promise.resolve(),
    })
    await settled()

    cancel()
    cancel()
    expect(unlisten).toHaveBeenCalledOnce()
  })

  it("reports a snapshot that could not be taken", async () => {
    const failed = vi.fn()
    const ready = vi.fn()

    attachThenAsk({
      listeners: [() => Promise.resolve(vi.fn())],
      ask: () => Promise.reject(new Error("available_update failed")),
      ready,
      failed,
    })
    await settled()

    // The listeners did attach, so an update announced from here on still
    // arrives; only the catch-up question was lost.
    expect(ready).toHaveBeenCalledOnce()
    expect(failed).toHaveBeenCalledOnce()
  })

  /** Both sides agree on whether anyone is still listening. */
  it("tells the listeners and ask alike when nobody is listening any more", async () => {
    let fromListener: (() => boolean) | undefined
    let fromAsk: (() => boolean) | undefined

    const cancel = attachThenAsk({
      listeners: [
        (live) => {
          fromListener = live
          return Promise.resolve(vi.fn())
        },
      ],
      ask: (live) => {
        fromAsk = live
        return Promise.resolve()
      },
    })
    await settled()

    expect(fromListener?.()).toBe(true)
    expect(fromAsk?.()).toBe(true)
    cancel()
    expect(fromListener?.()).toBe(false)
    expect(fromAsk?.()).toBe(false)
  })
})
