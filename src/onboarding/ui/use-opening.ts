import * as React from "react"
import { beatLength, nextBeat, type OpeningBeat } from "../model/opening"

const reducedMotionQuery = "(prefers-reduced-motion: reduce)"

function subscribeToReducedMotion(callback: () => void) {
  if (typeof matchMedia === "undefined") return () => {}
  const query = matchMedia(reducedMotionQuery)
  query.addEventListener("change", callback)
  return () => query.removeEventListener("change", callback)
}

/** The live reduced-motion preference. A page with no media queries moves. */
export function useReducedMotion(): boolean {
  return React.useSyncExternalStore(
    subscribeToReducedMotion,
    () => typeof matchMedia !== "undefined" && matchMedia(reducedMotionQuery).matches,
    () => false,
  )
}

/**
 * Walks the opening's beats (`model/opening.ts`) on a timer, once per mount.
 *
 * Under reduced motion there is no opening: the window is simply open, which
 * is the same end state the beats arrive at. A preference turned on halfway
 * through ends the opening there and then.
 */
export function useOpening(): OpeningBeat {
  const reduced = useReducedMotion()
  const [beat, setBeat] = React.useState<OpeningBeat>(reduced ? "open" : "glow")

  React.useEffect(() => {
    if (reduced) {
      setBeat("open")
      return
    }
    const length = beatLength(beat)
    if (length === undefined) return
    const timer = window.setTimeout(() => {
      setBeat(nextBeat)
    }, length)
    return () => window.clearTimeout(timer)
  }, [beat, reduced])

  return reduced ? "open" : beat
}
