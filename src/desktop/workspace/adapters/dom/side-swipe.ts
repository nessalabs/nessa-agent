/**
 * Reads a two-finger sideways swipe over the chat from the page's wheel
 * events (`model/side-swipe.ts` decides what counts). Only over the chat:
 * other surfaces keep the gesture for themselves, and a swipe that begins
 * over something that scrolls sideways (a wide table, a code block, a long
 * input) is that thing's scroll.
 */
import { useEffect, useRef, type RefObject } from "react"
import {
  beginsGesture,
  idleSwipe,
  swipeStep,
  type SwipeDirection,
  type SwipeState,
} from "../../model/side-swipe"

/** Where a swipe is read: the chat's panes. */
export const SWIPE_SURFACE = ".workspace-chat"

/** A line is about this many pixels, for a wheel that counts in lines. */
const LINE = 16

/**
 * Whether anything on the wheel's `path` before `surface` can scroll
 * sideways: wider than its box and set to scroll, or an input holding more
 * than it shows. The path is the event's composed one, so a scroller inside a
 * shadow root counts: a highlighted code block draws its lines in one, and
 * the event's own target there is only the root's host, which never scrolls.
 */
export function scrollsSideways(path: readonly EventTarget[], surface: Element): boolean {
  for (const at of path) {
    if (at === surface) return false
    if (!(at instanceof Element) || at.scrollWidth <= at.clientWidth + 1) continue
    if (at instanceof HTMLInputElement || at instanceof HTMLTextAreaElement) return true
    const overflow = getComputedStyle(at).overflowX
    if (overflow === "auto" || overflow === "scroll") return true
  }
  return false
}

/**
 * The wheel's sideways travel with x positive when the fingers moved left.
 * WebKit says whether the system reverses scrolling ("natural" scrolling, on
 * by default on a Mac); with it off the wheel's sign is the other way round.
 * Where it is not said, it is taken as on.
 */
function travel(event: WheelEvent): { x: number; y: number } {
  const scale = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? LINE : 1
  const natural =
    (event as WheelEvent & { webkitDirectionInvertedFromDevice?: boolean })
      .webkitDirectionInvertedFromDevice !== false
  return {
    x: event.deltaX * scale * (natural ? 1 : -1),
    y: event.deltaY * scale,
  }
}

/**
 * Calls `onSwipe` once per swipe over the chat inside `root`, while
 * `enabled`. The listener is passive: the chat's own scrolling is never held.
 */
export function useSideSwipe(
  root: RefObject<HTMLElement | null>,
  enabled: boolean,
  onSwipe: (direction: SwipeDirection) => void,
) {
  const latest = useRef(onSwipe)
  latest.current = onSwipe
  useEffect(() => {
    const scope = root.current
    if (!enabled || !scope) return
    let state: SwipeState = idleSwipe
    const onWheel = (event: WheelEvent) => {
      const target = event.target instanceof Element ? event.target : null
      const surface = target?.closest(SWIPE_SURFACE)
      const sample = { at: event.timeStamp, ...travel(event) }
      // Read only where the gesture begins: a swipe is the chat's or not
      // from its first event, whatever the pointer passes over after.
      const skip =
        beginsGesture(state, sample) &&
        (!target ||
          !surface ||
          // A pinch is a wheel with Control held; Shift turns a mouse's wheel sideways.
          event.ctrlKey ||
          event.shiftKey ||
          target.closest("[inert]") !== null ||
          scrollsSideways(event.composedPath(), surface))
      const next = swipeStep(state, sample, skip)
      state = next.state
      if (next.swiped) latest.current(next.swiped)
    }
    scope.addEventListener("wheel", onWheel, { passive: true })
    return () => scope.removeEventListener("wheel", onWheel)
  }, [root, enabled])
}
