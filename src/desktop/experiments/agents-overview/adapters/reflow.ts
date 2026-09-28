/**
 * The overview's list re-flowing, played back by transform: when a request
 * leaves or a session changes group, the page lays out at once and each item
 * travels from where it was drawn to where it now is (FLIP); an item that was
 * not there before fades in where it lands. Only transform and opacity move.
 *
 * Where each item was is kept as laid out within the list (which must be
 * positioned), so scrolling moves nothing, and taken again whenever the list's size changes without a render
 * — a request read in, a window resized — so a later re-flow starts from what
 * was really on screen. Durations are the window's tokens, which less motion
 * sets to zero; with zero there is nothing to play.
 */
import { useLayoutEffect, useRef, type RefObject } from "react"

const itemSelector = "[data-reflow]"

function token(element: Element, name: string, fallback: string): string {
  return getComputedStyle(element).getPropertyValue(name).trim() || fallback
}

/**
 * A duration token of the window's (`--desktop-base`, `--desktop-slow`) as
 * it applies to `element`, in milliseconds: zero where less motion asks.
 */
export function durationOf(element: Element | null, name: string): number {
  if (!element) return 0
  const value = token(element, name, "0ms")
  const number = Number.parseFloat(value)
  if (!Number.isFinite(number)) return 0
  return value.endsWith("ms") ? number : number * 1000
}

type Places = Map<string, { top: number; left: number }>

/**
 * Where an item is laid out within the list — never where a flight draws it,
 * so a re-flow asked for mid-flight starts from the layout, not a transform.
 */
function offsetWithin(
  item: HTMLElement,
  list: HTMLElement,
): { top: number; left: number } {
  let top = 0
  let left = 0
  let at: HTMLElement | null = item
  while (at && at !== list) {
    top += at.offsetTop
    left += at.offsetLeft
    const parent = at.offsetParent as HTMLElement | null
    // The list is positioned, so every chain ends at it; one that does not is measured from the page.
    if (parent && !list.contains(parent) && parent !== list) break
    at = parent
  }
  return { top, left }
}

function places(list: HTMLElement): Places {
  const found: Places = new Map()
  for (const item of list.querySelectorAll<HTMLElement>(itemSelector)) {
    const key = item.dataset.reflow
    if (key) found.set(key, offsetWithin(item, list))
  }
  return found
}

/**
 * Plays the list's re-flow after every render of the component that calls
 * it. Items carry `data-reflow` with a key of their own among the list.
 */
export function useReflow(list: RefObject<HTMLElement | null>): void {
  const before = useRef<Places | null>(null)

  useLayoutEffect(() => {
    const element = list.current
    if (!element) return
    const now = places(element)
    const was = before.current
    before.current = now
    // The first layout is the page arriving, which has its own motion.
    if (!was || typeof element.animate !== "function") return
    const duration = durationOf(element, "--desktop-slow")
    if (duration === 0) return
    const easing = token(element, "--desktop-out", "ease-out")
    for (const item of element.querySelectorAll<HTMLElement>(itemSelector)) {
      const key = item.dataset.reflow
      if (!key) continue
      const to = now.get(key)
      const from = was.get(key)
      if (!to) continue
      if (!from) {
        item.animate(
          [
            { opacity: 0, transform: "translateY(6px)" },
            { opacity: 1, transform: "none" },
          ],
          { duration, easing, delay: duration * 0.5, fill: "backwards" },
        )
        continue
      }
      const dy = from.top - to.top
      const dx = from.left - to.left
      if (Math.abs(dy) < 0.5 && Math.abs(dx) < 0.5) continue
      item.animate(
        [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
        { duration, easing },
      )
    }
  })

  // Sizes that change without a render still move what follows them.
  useLayoutEffect(() => {
    const element = list.current
    if (!element || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(() => {
      before.current = places(element)
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [list])
}
