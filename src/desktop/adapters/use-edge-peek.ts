import { useCallback, useEffect, useReducer, useRef } from "react"
import { edgePeekHidden, stepEdgePeek } from "../model/edge-peek"

/** Resting on the edge this long reveals the sidebar; a pass-through does not. */
const REVEAL_MS = 150
/** Away from the revealed sidebar this long hides it; coming back cancels. */
const HIDE_MS = 350

/**
 * Runs the edge-peek state machine (`model/edge-peek.ts`) against the clock.
 * `enabled` is false whenever the sidebar is docked open or the panel is
 * maximized, so a reveal never sits over the real sidebar.
 */
export function useEdgePeek(enabled: boolean) {
  const [state, send] = useReducer(stepEdgePeek, edgePeekHidden)
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)

  useEffect(() => {
    clearTimeout(timer.current)
    if (state.pending === "reveal")
      timer.current = setTimeout(() => send("reveal-due"), REVEAL_MS)
    if (state.pending === "hide")
      timer.current = setTimeout(() => send("hide-due"), HIDE_MS)
    return () => clearTimeout(timer.current)
  }, [state])

  useEffect(() => {
    if (!enabled) send("dismiss")
  }, [enabled])

  useEffect(() => {
    if (!state.shown) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") send("dismiss")
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [state.shown])

  const enter = useCallback(() => {
    if (enabled) send("enter")
  }, [enabled])
  const leave = useCallback(() => send("leave"), [])

  return { shown: enabled && state.shown, enter, leave }
}
