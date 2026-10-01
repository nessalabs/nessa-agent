import { useCallback, useEffect, useRef, useState } from "react"
import { inModal } from "./modal"
import {
  edgePeekHidden,
  stepEdgePeek,
  type EdgePeek,
  type EdgePeekEvent,
} from "../model/edge-peek"

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
  // The model's whole state, stepped on every event; the page is drawn again
  // only when what it draws changes — every press and release in the window
  // reaches the model, and most change nothing on screen.
  const model = useRef<EdgePeek>(edgePeekHidden)
  const [state, setState] = useState<EdgePeek>(edgePeekHidden)
  const send = useCallback((event: EdgePeekEvent) => {
    const was = model.current
    const next = stepEdgePeek(was, event)
    model.current = next
    if (
      next.shown !== was.shown ||
      next.pending !== was.pending ||
      next.handedOff !== was.handedOff
    )
      setState(next)
  }, [])
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const pending = state.pending

  useEffect(() => {
    clearTimeout(timer.current)
    const due = {
      reveal: "reveal-due",
      hide: "hide-due",
      handoff: "handoff-due",
    } as const
    const delay = { reveal: REVEAL_MS, hide: HIDE_MS, handoff: HANDOFF_MS } as const
    if (pending) timer.current = setTimeout(() => send(due[pending]), delay[pending])
    return () => clearTimeout(timer.current)
    // Timed from when it became pending: where the pointer is while a
    // handoff plays does not start its clock again.
  }, [pending, send])

  useEffect(() => {
    if (!enabled) send(docked ? "dock" : "dismiss")
  }, [enabled, docked, send])

  // A shown reveal, not handing off to the docked sidebar, takes Escape and
  // keeps it, after a menu or a dialog's own: heard first (capture) and
  // marked handled, so whatever else answers Escape — the overview, a widget
  // (ADR 326) — leaves it be. Read as rendered, by one listener for the
  // hook's life: the reveal is on the page from its commit, so its Escape is
  // too, not an effect later.
  const ownsEscape = state.shown && state.pending !== "handoff"
  const owns = useRef(ownsEscape)
  owns.current = ownsEscape
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !owns.current || event.defaultPrevented) return
      if (inModal(event.target)) return
      event.preventDefault()
      send("dismiss")
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [send])

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
    [enabled, send],
  )
  // A press freezes the peek (`model/edge-peek.ts`): the model is told of
  // every press and release, and of a release the page missed — the button
  // let go outside the window — at the next enter or leave with no button
  // held, or the window's blur. An enter or leave with a button held and no
  // press seen (one begun outside the window) is a press.
  useEffect(() => {
    const press = () => send("press")
    const release = () => send("release")
    window.addEventListener("pointerdown", press, true)
    window.addEventListener("pointerup", release, true)
    window.addEventListener("pointercancel", release, true)
    window.addEventListener("blur", release)
    return () => {
      window.removeEventListener("pointerdown", press, true)
      window.removeEventListener("pointerup", release, true)
      window.removeEventListener("pointercancel", release, true)
      window.removeEventListener("blur", release)
    }
  }, [send])
  const pointerAt = useCallback(
    (now: boolean, event?: { buttons: number }) => {
      if (event) send(event.buttons === 0 ? "release" : "press")
      held("pointer", now)
    },
    [held, send],
  )
  const enter = useCallback(
    (event?: { buttons: number }) => pointerAt(true, event),
    [pointerAt],
  )
  const leave = useCallback(
    (event?: { buttons: number }) => pointerAt(false, event),
    [pointerAt],
  )
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
