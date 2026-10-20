/**
 * Fitting the workspace's side columns to the window: the layouts draw them
 * from the window's width (`adapters/window-width.ts`), and fold them when
 * it grows too narrow for the panes beside them (`model/window-fit.ts`).
 */
import { useLayoutEffect, useRef } from "react"

/**
 * Calls `fit` with the window's width when the window is resized, when the
 * number of pane columns changes, and once on mount — only then, so a column
 * the person opened by hand in a small window stays open until either
 * changes, and one the window folded for room returns once there is room.
 *
 * A resize, and the first mount, fold before paint: the fold is that frame,
 * and nothing waits for a later effect that a resize observer's synchronous
 * render would flush out of turn. A change in the number of pane columns
 * folds on the next frame. The click that opened the pane has already spent
 * its script, and the grid paints the new boxes the frame after the column
 * title has glided (`SplitPanes`).
 */
export function useFitOnResize(
  windowWidth: number,
  columns: number,
  fit: (windowWidth: number) => void,
) {
  const latest = useRef(fit)
  latest.current = fit
  const seen = useRef<{ width: number; columns: number } | null>(null)
  useLayoutEffect(() => {
    const previous = seen.current
    seen.current = { width: windowWidth, columns }
    if (previous === null || previous.width !== windowWidth) {
      latest.current(windowWidth)
      return
    }
    if (previous.columns === columns) return
    const id = requestAnimationFrame(() => latest.current(windowWidth))
    return () => cancelAnimationFrame(id)
  }, [windowWidth, columns])
}
