import { useEffect, useState } from "react"

/** How long work must last before it is shown: a send refused at once never flickers. */
export const workingShowAfterMs = 300
/** How long shown work stays at least, so a sign that appears does not blink away. */
export const workingHoldMs = 600

/**
 * Whether to show that something works: only once it has worked for
 * `workingShowAfterMs`, and then for at least `workingHoldMs`. A turn that
 * starts and ends within a moment (a send the host refuses) shows nothing.
 * `startedAt`, where the work's own start is known, counts from it, so work
 * that has already lasted shows at once.
 */
export function useSettledWorking(working: boolean, startedAt?: number): boolean {
  const lasted = (now: number) =>
    startedAt !== undefined && now - startedAt >= workingShowAfterMs
  const [shown, setShown] = useState(() => working && lasted(Date.now()))
  const [since, setSince] = useState(() => Date.now())
  useEffect(() => {
    if (working && !shown) {
      const wait =
        startedAt === undefined
          ? workingShowAfterMs
          : Math.max(0, startedAt + workingShowAfterMs - Date.now())
      const timer = window.setTimeout(() => {
        setShown(true)
        setSince(Date.now())
      }, wait)
      return () => window.clearTimeout(timer)
    }
    if (!working && shown) {
      const left = Math.max(0, since + workingHoldMs - Date.now())
      const timer = window.setTimeout(() => setShown(false), left)
      return () => window.clearTimeout(timer)
    }
    return undefined
  }, [working, shown, since, startedAt])
  // Work known to have lasted shows on this paint, not after a timer.
  return shown || (working && lasted(Date.now()))
}
