import { useLayoutEffect, useState, type RefObject } from "react"
import { flushSync } from "react-dom"

/**
 * Whether an element is laid out at least `min` pixels wide, followed as it
 * resizes — the window, a side column opening. It is measured only by the
 * `ResizeObserver`, after the page's own layout: never by a read that makes
 * the page lay out early, then again. Observe an element that is on the page
 * before what reads the answer appears (the overview's layer, mounted with
 * the workspace), so the answer is already known when it opens, and whose
 * size does not follow what it holds, so a change of answer cannot resize it
 * again. Unmeasured (no `ResizeObserver`), it is narrow.
 */
export function useAtLeastWide(
  element: RefObject<HTMLElement | null>,
  min: number,
): boolean {
  const [wide, setWide] = useState(false)
  useLayoutEffect(() => {
    const target = element.current
    if (!target || typeof ResizeObserver === "undefined") return
    let last: boolean | null = null
    const observer = new ResizeObserver(([entry]) => {
      const next =
        (entry.borderBoxSize?.[0]?.inlineSize ?? entry.contentRect.width) >= min
      if (next === last) return
      last = next
      // Decided before the frame paints, so no frame shows the other arrangement.
      flushSync(() => setWide(next))
    })
    observer.observe(target)
    return () => observer.disconnect()
  }, [element, min])
  return wide
}
