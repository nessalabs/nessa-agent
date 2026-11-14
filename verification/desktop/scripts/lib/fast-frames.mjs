/** Install once in a fresh page for the focus check's 8 ms frame probe. */
export function installFastFrames() {
  const queued = new Map()
  const requestNative = window.requestAnimationFrame.bind(window)
  const cancelNative = window.cancelAnimationFrame.bind(window)
  window.requestAnimationFrame = (callback) => {
    // Share the browser's handle allocator with callbacks retained before
    // installation. Each request adds a native registration/cancellation;
    // the no-op is canceled in this turn, before the timer is registered.
    const id = requestNative(() => {})
    cancelNative(id)
    const timer = window.setTimeout(() => {
      queued.delete(id)
      callback(performance.now())
    }, 8)
    queued.set(id, timer)
    return id
  }
  window.cancelAnimationFrame = (id) => {
    const timer = queued.get(id)
    if (timer !== undefined) {
      window.clearTimeout(timer)
      queued.delete(id)
    }
    cancelNative(id)
  }
}
