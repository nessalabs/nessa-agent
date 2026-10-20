import * as React from "react"
import { onRuntimeCopy, type RuntimeCopy } from "../../host"

/** The latest private-runtime copy report, if the host has sent one.
 *
 * Absent until the first event. `settled` stays in the value so a caller can
 * tell reporting stopped; the agent list does not read this. */
export function useRuntimeCopy(): RuntimeCopy | undefined {
  const [progress, setProgress] = React.useState<RuntimeCopy>()
  React.useEffect(() => {
    let stop = () => {}
    let cancelled = false
    void onRuntimeCopy((next) => {
      if (!cancelled) setProgress(next)
    }).then((unlisten) => {
      if (cancelled) unlisten()
      else stop = unlisten
    })
    return () => {
      cancelled = true
      stop()
    }
  }, [])
  return progress
}
