import { describe, expect, it, vi } from "vitest"
import { attachThenAsk } from "./update-subscription"

function deferred<T>() {
  let settle!: (value: T) => void
  const promise = new Promise<T>((done) => {
    settle = done
  })
  return { promise, settle }
}

describe("attachThenAsk", () => {
  /**
   * The fault this exists to prevent: the host announced an update while the
   * listeners were still being attached, the question had already been asked
   * and answered "nothing", and the panel never heard about it again.
   */
  it("asks only once every listener is really attached", async () => {
    const attaching = deferred<(() => void)[]>()
    const asked = vi.fn(() => Promise.resolve())

    attachThenAsk({ attach: () => attaching.promise, ask: asked })
    await Promise.resolve()
    expect(asked).not.toHaveBeenCalled()

    attaching.settle([])
    await attaching.promise
    await Promise.resolve()
    expect(asked).toHaveBeenCalledOnce()
  })

  it("says the host can be heard from before asking, and not before that", async () => {
    const attaching = deferred<(() => void)[]>()
    const order: string[] = []

    attachThenAsk({
      attach: () => attaching.promise,
      ask: async () => {
        order.push("ask")
      },
      ready: () => order.push("ready"),
    })
    await Promise.resolve()
    expect(order).toEqual([])

    attaching.settle([])
    await attaching.promise
    await Promise.resolve()
    expect(order).toEqual(["ready", "ask"])
  })

  /** Unmounted while registering: the listeners arrive with nobody wanting them. */
  it("takes down listeners that resolve after it was cancelled", async () => {
    const attaching = deferred<(() => void)[]>()
    const unlisten = vi.fn()
    const asked = vi.fn(() => Promise.resolve())
    const ready = vi.fn()

    const cancel = attachThenAsk({
      attach: () => attaching.promise,
      ask: asked,
      ready,
    })
    cancel()
    attaching.settle([unlisten, unlisten])
    await attaching.promise
    await Promise.resolve()

    expect(unlisten).toHaveBeenCalledTimes(2)
    expect(asked).not.toHaveBeenCalled()
    expect(ready).not.toHaveBeenCalled()
  })

  it("takes down listeners it is holding when cancelled", async () => {
    const unlisten = vi.fn()
    const cancel = attachThenAsk({
      attach: () => Promise.resolve([unlisten]),
      ask: () => Promise.resolve(),
    })
    await Promise.resolve()
    await Promise.resolve()

    cancel()
    expect(unlisten).toHaveBeenCalledOnce()
    // And a second cancel does not take down a listener twice.
    cancel()
    expect(unlisten).toHaveBeenCalledOnce()
  })

  /** Both sides agree on whether anyone is still listening. */
  it("tells attach and ask alike when nobody is listening any more", async () => {
    let fromAttach: (() => boolean) | undefined
    let fromAsk: (() => boolean) | undefined

    const cancel = attachThenAsk({
      attach: (live) => {
        fromAttach = live
        return Promise.resolve([])
      },
      ask: (live) => {
        fromAsk = live
        return Promise.resolve()
      },
    })
    await Promise.resolve()
    await Promise.resolve()

    expect(fromAttach?.()).toBe(true)
    expect(fromAsk?.()).toBe(true)
    cancel()
    expect(fromAttach?.()).toBe(false)
    expect(fromAsk?.()).toBe(false)
  })
})
