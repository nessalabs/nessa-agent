import { useCallback, useEffect, useReducer, useRef } from "react"
import { edgePeekHidden, stepEdgePeek } from "../model/edge-peek"

/** Resting on the edge this long reveals the sidebar; a pass-through does not. */
const REVEAL_MS = 150
/** Away from the revealed sidebar this long hides it; coming back cancels. */
const HIDE_MS = 350
/** Long enough for the docked sidebar's open transition (340ms) to finish. */
const HANDOFF_MS = 380

/**
 * Runs the edge-peek state machine (`model/edge-peek.ts`) against the clock:
 * the one Dock-style reveal of a folded sidebar, used by the classic shell,
 * both workspace layouts and Settings. `enabled` is false whenever the
 * sidebar is drawn (or, in the classic shell, the panel is maximized).
 * Becoming docked while revealed is a handoff; anything else that disables
 * the reveal dismisses it.
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

  // The pointer and keyboard focus each hold the reveal open: it hides once
  // neither is inside — the edge strip or the revealed sidebar — so choosing
  // something there keeps it shown until the pointer leaves, and Tabbing
  // through it keeps it shown while focus is there. Focus a press gave does
  // not hold it: the pointer leaving is what a press means to end.
  const inside = useRef({ pointer: false, focus: false })
  const held = useCallback(
    (part: "pointer" | "focus", now: boolean) => {
      const was = inside.current.pointer || inside.current.focus
      inside.current = { ...inside.current, [part]: now }
      const is = inside.current.pointer || inside.current.focus
      if (is && !was && enabled) send("enter")
      else if (!is && was) send("leave")
    },
    [enabled],
  )
  const enter = useCallback(() => held("pointer", true), [held])
  const leave = useCallback(() => held("pointer", false), [held])
  const focusIn = useCallback(
    (event: { target: EventTarget }) => {
      if (event.target instanceof Element && event.target.matches(":focus-visible"))
        held("focus", true)
    },
    [held],
  )
  const focusOut = useCallback(
    (event: { currentTarget: Element; relatedTarget: EventTarget | null }) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null))
        held("focus", false)
    },
    [held],
  )

  return {
    shown: state.shown,
    handingOff: state.pending === "handoff",
    handedOff: state.handedOff,
    enter,
    leave,
    /** For the revealed sidebar: pointer and focus both hold it open. */
    holders: {
      onPointerEnter: enter,
      onPointerLeave: leave,
      onFocus: focusIn,
      onBlur: focusOut,
    },
  }
}

export type EdgePeekControls = ReturnType<typeof useEdgePeek>
