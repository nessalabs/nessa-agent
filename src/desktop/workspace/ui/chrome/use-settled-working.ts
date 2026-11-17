import { useEffect, useState } from "react"
import { useClock, useElapsed } from "../../../../clock"

/** How long work must last before it is shown: a send refused at once never flickers. */
export const workingShowAfterMs = 300
/** How long shown work stays at least, so a sign that appears does not blink away. */
export const workingHoldMs = 600

/**
 * Whether to show that something works: only once it has worked for
 * `workingShowAfterMs`, and then for at least `workingHoldMs`. A turn that
 * starts and ends within a moment (a send the host refuses) shows nothing.
 * `startedAt`, where the work's own start is known (on the injected clock,
 * as `Activity.since` is), counts from it, so work that has already lasted
 * shows at once. The hold is a duration, measured on the monotonic clock.
 */
export function useSettledWorking(working: boolean, startedAt?: number): boolean {
  const now = useClock()
  const elapsed = useElapsed()
  const lasted = () => startedAt !== undefined && now() - startedAt >= workingShowAfterMs
  const [shown, setShown] = useState(() => working && lasted())
  const [shownAt, setShownAt] = useState(() => elapsed())
  useEffect(() => {
    if (working && !shown) {
      const wait =
        startedAt === undefined
          ? workingShowAfterMs
          : Math.max(0, startedAt + workingShowAfterMs - now())
      const timer = window.setTimeout(() => {
        setShown(true)
        setShownAt(elapsed())
      }, wait)
      return () => window.clearTimeout(timer)
    }
    if (!working && shown) {
      const left = Math.max(0, shownAt + workingHoldMs - elapsed())
      const timer = window.setTimeout(() => setShown(false), left)
      return () => window.clearTimeout(timer)
    }
    return undefined
  }, [working, shown, shownAt, startedAt, now, elapsed])
  // Work known to have lasted shows on this paint, not after a timer.
  return shown || (working && lasted())
}
