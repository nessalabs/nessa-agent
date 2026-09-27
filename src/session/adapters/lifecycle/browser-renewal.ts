/** Renew only while this browser is visible. Failed checks retry without signing out. */
export function maintainBrowserSession(options: {
  check: () => Promise<boolean>
  ended: () => void
  visible: () => boolean
  subscribe: (check: () => void) => () => void
  schedule: (check: () => void) => () => void
}) {
  let disposed = false
  let pending = false
  const check = () => {
    if (disposed || pending || !options.visible()) return
    pending = true
    void options
      .check()
      .then((valid) => {
        if (!disposed && !valid) options.ended()
      })
      .catch(() => {
        // A service outage is not a confirmed logout. The next check retries.
      })
      .finally(() => {
        pending = false
      })
  }
  const unsubscribe = options.subscribe(check)
  const cancel = options.schedule(check)
  return () => {
    disposed = true
    unsubscribe()
    cancel()
  }
}
