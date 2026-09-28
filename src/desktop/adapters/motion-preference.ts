import { useLayoutEffect, useSyncExternalStore } from "react"
import { motionInEffect, parseMotionChoice } from "../model/motion"
import { storedPreference } from "./stored-preference"

/**
 * The motion the person chose (`model/motion.ts`), remembered like the
 * window's other preferences, and the motion in effect, carried on the
 * page's root as `data-motion` for the stylesheet and every script.
 */
const motionPreference = storedPreference({
  key: "nessa.desktop.motion",
  event: "nessa:desktop-motion",
  parse: parseMotionChoice,
})

export const useMotionPreference = motionPreference.usePreference

const systemQuery = "(prefers-reduced-motion: reduce)"

function subscribeToSystem(listener: () => void) {
  if (typeof matchMedia === "undefined") return () => {}
  const query = matchMedia(systemQuery)
  query.addEventListener("change", listener)
  return () => query.removeEventListener("change", listener)
}

const systemReduces = () =>
  typeof matchMedia !== "undefined" && matchMedia(systemQuery).matches

/**
 * Keeps the root's `data-motion` to the motion in effect, before the first
 * paint and whenever the choice or the system's setting changes. Mounted
 * once, by the window.
 */
export function useMotionInEffect(): void {
  const [choice] = useMotionPreference()
  const system = useSyncExternalStore(subscribeToSystem, systemReduces, () => false)
  useLayoutEffect(() => {
    document.documentElement.dataset.motion = motionInEffect(choice, system)
  }, [choice, system])
}

/** Whether the window moves less now: for motion no duration token covers, such as scrolling. */
export function reducedMotion(): boolean {
  return (
    typeof document !== "undefined" &&
    document.documentElement.dataset.motion === "reduced"
  )
}
