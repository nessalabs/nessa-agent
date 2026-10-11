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
    let latest = false
    let frame: number | null = null
    const observer = new ResizeObserver(([entry]) => {
      const next =
        (entry.borderBoxSize?.[0]?.inlineSize ?? entry.contentRect.width) >= min
      latest = next
      if (frame !== null || next === last) return
      // Flushing React during observer delivery can also mount neighboring
      // observers from pending updates and invalidate this broadcast (#693).
      frame = requestAnimationFrame(() => {
        frame = null
        if (latest === last) return
        last = latest
        flushSync(() => setWide(latest))
      })
    })
    observer.observe(target)
    return () => {
      observer.disconnect()
      if (frame !== null) cancelAnimationFrame(frame)
    }
  }, [element, min])
  return wide
}
