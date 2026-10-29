import * as React from "react"
import { setupTrayPoint } from "../../host"
import { FLIGHT_MS, flightFrames, flightTarget } from "../model/opening"
import { useReducedMotion } from "./use-opening"

/** Longest the flight waits for the host to say where the menu bar icon is.
 * Past it the window flies to the corner instead: an answer that slow is no
 * better than none, and the flight should start while the press is fresh. */
const TRAY_ASK_MS = 400

/** Longest a flight that has started may take before setup lands it anyway:
 * setup must never be held open by an animation that stalls. */
const FLIGHT_DEADLINE_MS = FLIGHT_MS + 600

/**
 * The finished setup window gathers into a small light and flies into the
 * menu bar, before setup hands over.
 *
 * Only a finish flies. Leaving setup part-way is not an arrival anywhere, and
 * stays as instant as the close button promises. Under reduced motion, or
 * without Web Animations, there is no flight: setup hands over at once.
 *
 * `flying` is derived in render rather than set by an effect, so the render
 * that first reports a finish already says the window is still in flight —
 * the handoff, which waits on it, never sees a frame in which it is not.
 */
export function useSetupFlight(
  finished: boolean,
  shell: React.RefObject<HTMLElement | null>,
): { flying: boolean } {
  const reduced = useReducedMotion()
  const [landed, setLanded] = React.useState(false)
  const canAnimate =
    typeof document !== "undefined" && typeof document.body?.animate === "function"
  const flying = finished && !landed && !reduced && canAnimate

  React.useEffect(() => {
    if (!flying) return
    const element = shell.current
    if (!element) {
      setLanded(true)
      return
    }
    let live = true
    let done = false
    let animation: Animation | undefined
    let deadline: ReturnType<typeof setTimeout> | undefined
    const land = () => {
      if (!live || done) return
      done = true
      setLanded(true)
    }
    const fly = (tray: { x: number; y: number } | undefined) => {
      if (!live || done || deadline !== undefined) return
      clearTimeout(asked)
      // Counted from takeoff, so a slow answer about the tray never cuts the
      // flight off mid-air; set first, so a flight that cannot start still
      // lands.
      deadline = setTimeout(land, FLIGHT_DEADLINE_MS)
      const box = element.getBoundingClientRect()
      const target = flightTarget(tray, { width: innerWidth, height: innerHeight })
      try {
        animation = element.animate(flightFrames(box, target), {
          duration: FLIGHT_MS,
          fill: "forwards",
        })
      } catch {
        land()
        return
      }
      animation.finished.then(land, land)
    }
    // Only ever run from this timer or the answer, both after it is set.
    const asked = setTimeout(() => fly(undefined), TRAY_ASK_MS)
    setupTrayPoint().then(fly, () => fly(undefined))
    return () => {
      live = false
      clearTimeout(asked)
      clearTimeout(deadline)
      // A flight that landed stays where it landed: the window closes over it.
      // One cut short goes back, so a window still on screen is whole.
      if (!done) animation?.cancel()
    }
  }, [flying, shell])

  // A setup that is back on screen flies again when it next finishes.
  React.useEffect(() => {
    if (!finished) setLanded(false)
  }, [finished])

  return { flying }
}
