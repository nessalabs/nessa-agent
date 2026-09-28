import { useLayoutEffect, useState, type RefObject } from "react"
import { flushSync } from "react-dom"

/**
 * Whether an element is laid out at least `min` pixels wide, followed as it
 * resizes — the window, a side column opening. It starts from `hint`, a width
 * the element will have at least (what its place measured before it
 * appeared), and is measured only by the `ResizeObserver`, after the page's
 * own layout: never by a read that makes the page lay out early, then again.
 * Unmeasured (no `ResizeObserver`), it is what the hint says.
 */
export function useAtLeastWide(
  element: RefObject<HTMLElement | null>,
  min: number,
  hint = 0,
): boolean {
  const [wide, setWide] = useState(hint >= min)
  useLayoutEffect(() => {
    const target = element.current
    if (!target || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(([entry]) => {
      const next =
        (entry.borderBoxSize?.[0]?.inlineSize ?? entry.contentRect.width) >= min
      // Decided before the frame paints, so no frame shows the other arrangement.
      flushSync(() => setWide(next))
    })
    observer.observe(target)
    return () => observer.disconnect()
  }, [element, min])
  return wide
}
