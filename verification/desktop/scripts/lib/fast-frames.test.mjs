import assert from "node:assert/strict"
import { test } from "node:test"
import { installFastFrames } from "./fast-frames.mjs"

function provider(t) {
  const previous = globalThis.window
  const native = new Map(),
    timers = new Map(),
    events = []
  let nextNative = 0
  const window = {
    requestAnimationFrame(callback) {
      assert.equal(this, window)
      const id = ++nextNative
      native.set(id, callback)
      events.push(["native request", id])
      return id
    },
    cancelAnimationFrame(id) {
      assert.equal(this, window)
      native.delete(id)
      events.push(["native cancel", id])
    },
    setTimeout(callback, delay) {
      let id = 1
      while (timers.has(id)) id++
      timers.set(id, callback)
      events.push(["timer", id, delay])
      return id
    },
    clearTimeout(id) {
      timers.delete(id)
      events.push(["timer cancel", id])
    },
  }
  const nativeRequest = window.requestAnimationFrame.bind(window)
  globalThis.window = window
  t.after(() => {
    if (previous === undefined) delete globalThis.window
    else globalThis.window = previous
  })
  return {
    window,
    nativeRequest,
    events,
    native,
    timers,
    fireNative() {
      for (const [id, callback] of [...native]) {
        if (!native.has(id)) continue
        native.delete(id)
        callback(123)
      }
    },
    fireTimers() {
      for (const [id, callback] of [...timers]) {
        if (!timers.has(id)) continue
        callback()
        timers.delete(id)
      }
    },
  }
}

for (const retired of [false, true])
  test(`canceling a ${retired ? "retired" : "pending"} native handle preserves a later shim frame`, (t) => {
    const p = provider(t)
    let nativeCalls = 0,
      shimCalls = 0
    const old = p.nativeRequest(() => nativeCalls++)
    if (retired) p.fireNative()
    installFastFrames()
    const current = p.window.requestAnimationFrame(() => shimCalls++)
    assert.notEqual(current, old)
    p.window.cancelAnimationFrame(old)
    p.fireTimers()
    p.fireNative()
    assert.equal(shimCalls, 1)
    assert.equal(nativeCalls, retired ? 1 : 0)
  })

test("each shim timer uses the allocated handle and cancels its placeholder first", (t) => {
  const p = provider(t)
  installFastFrames()
  const times = []
  const before = performance.now()
  const handle = p.window.requestAnimationFrame((at) => times.push(at))
  assert.deepEqual(p.events, [
    ["native request", handle],
    ["native cancel", handle],
    ["timer", 1, 8],
  ])
  assert.equal(p.native.size, 0)
  p.fireNative()
  assert.equal(times.length, 0, "the placeholder must not deliver")
  p.fireTimers()
  assert.equal(times.length, 1)
  assert.ok(times[0] >= before && times[0] <= performance.now())
  p.fireTimers()
  assert.equal(times.length, 1)
})

test("canceling a shim timer preserves work through a retained native request", (t) => {
  const p = provider(t)
  installFastFrames()
  let shimCalls = 0,
    nativeCalls = 0
  const shim = p.window.requestAnimationFrame(() => shimCalls++)
  const native = p.nativeRequest(() => nativeCalls++)
  p.window.cancelAnimationFrame(shim)
  p.window.cancelAnimationFrame(shim)
  p.fireTimers()
  p.fireNative()
  assert.equal(shimCalls, 0)
  assert.equal(nativeCalls, 1)
  assert.ok(p.events.some((event) => event[0] === "native cancel" && event[1] === shim))
  assert.notEqual(shim, native)
})

test("retired shim cleanup cannot cancel a later timer that reused its timer id", (t) => {
  const p = provider(t)
  installFastFrames()
  let calls = 0
  const retired = p.window.requestAnimationFrame(() => calls++)
  p.fireTimers()
  p.window.requestAnimationFrame(() => calls++)
  p.window.cancelAnimationFrame(retired)
  p.fireTimers()
  assert.equal(calls, 2)
})

test("the timer mapping retires before a reentrant callback requests and cancels work", (t) => {
  const p = provider(t)
  installFastFrames()
  let handle,
    later,
    calls = 0
  handle = p.window.requestAnimationFrame(() => {
    calls++
    later = p.window.requestAnimationFrame(() => calls++)
    p.window.cancelAnimationFrame(handle)
    assert.equal(p.events.filter(([kind]) => kind === "timer cancel").length, 0)
  })
  p.fireTimers()
  assert.notEqual(handle, later)
  p.fireTimers()
  p.fireNative()
  assert.equal(calls, 2)
})

test("mixed cancellation keeps each remaining native and shim callback", (t) => {
  const p = provider(t)
  const calls = []
  const old = p.nativeRequest(() => calls.push("old"))
  installFastFrames()
  const removed = p.window.requestAnimationFrame(() => calls.push("removed"))
  p.window.requestAnimationFrame(() => calls.push("kept"))
  p.nativeRequest(() => calls.push("native"))
  p.window.cancelAnimationFrame(removed)
  p.window.cancelAnimationFrame(old)
  p.fireTimers()
  p.fireNative()
  assert.deepEqual(calls, ["kept", "native"])
})
