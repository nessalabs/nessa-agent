import { useCallback, useEffect, useReducer, useRef } from "react"
import { edgePeekHidden, stepEdgePeek } from "../model/edge-peek"

/** Resting on the edge this long reveals the sidebar; a pass-through does not. */
const REVEAL_MS = 150
/** Away from the revealed sidebar this long hides it; coming back cancels. */
const HIDE_MS = 350
/** Long enough for the docked sidebar's open transition (340ms) to finish. */
const HANDOFF_MS = 380

/**
 * Runs the edge-peek state machine (`model/edge-peek.ts`) against the clock.
 * `enabled` is false whenever the sidebar is docked open or the panel is
 * maximized. Becoming docked while revealed is a handoff; anything else
 * that disables the reveal dismisses it.
 */
export function useEdgePeek(enabled: boolean, docked: boolean) {
  const [state, send] = useReducer(stepEdgePeek, edgePeekHidden)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)

  useEffect(() => {
    clearTimeout(timer.current)
    const due = {
      reveal: "reveal-due",
      hide: "hide-due",
      handoff: "handoff-due",
    } as const
    const delay = { reveal: REVEAL_MS, hide: HIDE_MS, handoff: HANDOFF_MS } as const
    const pending = state.pending
    if (pending) timer.current = setTimeout(() => send(due[pending]), delay[pending])
    return () => clearTimeout(timer.current)
  }, [state])

  useEffect(() => {
    if (!enabled) send(docked ? "dock" : "dismiss")
  }, [enabled, docked])

  useEffect(() => {
    if (!state.shown || state.pending === "handoff") return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") send("dismiss")
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [state.shown, state.pending])

  const enter = useCallback(() => {
    if (enabled) send("enter")
  }, [enabled])
  const leave = useCallback(() => send("leave"), [])

  return {
    shown: state.shown,
    handingOff: state.pending === "handoff",
    handedOff: state.handedOff,
    enter,
    leave,
  }
}
