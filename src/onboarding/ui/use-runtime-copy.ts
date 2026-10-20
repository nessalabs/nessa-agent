import * as React from "react"
import { onRuntimeCopy, runtimeCopy, type RuntimeCopy } from "../../host"

/** The latest private-runtime copy report, if the host has one.
 *
 * Subscribe first, then read the retained snapshot. An event that arrives
 * during that read is kept; the snapshot is not applied over it. A failed
 * subscription still reads the snapshot. Absent when the host has not
 * reported. `settled` stays in the value so a caller can tell reporting
 * stopped; the agent list does not read this. */
export function useRuntimeCopy(): RuntimeCopy | undefined {
  const [progress, setProgress] = React.useState<RuntimeCopy>()
  React.useEffect(() => {
    let stop = () => {}
    let cancelled = false
    let seen = false
    void (async () => {
      try {
        const unlisten = await onRuntimeCopy((next) => {
          seen = true
          if (!cancelled) setProgress(next)
        })
        if (cancelled) {
          unlisten()
          return
        }
        stop = unlisten
      } catch {
        // The listener did not register. The retained read is still the report.
        if (cancelled) return
      }
      try {
        const snapshot = await runtimeCopy()
        if (!cancelled && !seen && snapshot) setProgress(snapshot)
      } catch {
        // A subscribed event is the report. A failed read must not clear it.
      }
    })()
    return () => {
      cancelled = true
      stop()
    }
  }, [])
  return progress
}
