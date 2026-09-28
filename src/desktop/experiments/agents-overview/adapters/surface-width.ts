import { useLayoutEffect, useState, type RefObject } from "react"

/**
 * Whether an element is laid out at least `min` pixels wide, followed as it
 * resizes — the window, a side column opening. Unmeasured (no layout, no
 * `ResizeObserver`) it is not.
 */
export function useAtLeastWide(
  element: RefObject<HTMLElement | null>,
  min: number,
): boolean {
  const [wide, setWide] = useState(false)
  useLayoutEffect(() => {
    const target = element.current
    if (!target) return
    const measure = () => setWide(target.offsetWidth >= min)
    measure()
    if (typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(measure)
    observer.observe(target)
    return () => observer.disconnect()
  }, [element, min])
  return wide
}
