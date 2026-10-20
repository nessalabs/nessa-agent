/**
 * Fitting the workspace's side columns to the window: the layouts draw them
 * from the window's width (`adapters/window-width.ts`), and fold them when
 * it grows too narrow for the panes beside them (`model/window-fit.ts`).
 */
import { useLayoutEffect, useRef, useState } from "react"

/**
 * Calls `fit` with the window's width when the window is resized, when the
 * number of pane columns changes, and once on mount — only then, so a column
 * the person opened by hand in a small window stays open until either
 * changes, and one the window folded for room returns once there is room.
 *
 * A resize, and the first mount, fold before paint: the fold is that frame,
 * and nothing waits for a later effect that a resize observer's synchronous
 * render would flush out of turn. A change in the number of pane columns
 * folds on the next frame. A pane that takes the sidebar's spare room has
 * already folded it in state (`arranged`); what the page paints of that
 * fold waits the same frame (`useDrawnAfterColumnChange`).
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

/**
 * The side column the page paints. A resize and a person's toggle paint
 * before the browser does. A change in the number of pane columns paints on
 * the next frame: the click that opened the pane folded the column in the
 * same dispatch (`arranged`), and painting that fold in the click walks the
 * document (`window-width.test.tsx`).
 */
export function useDrawnAfterColumnChange(open: boolean, columns: number): boolean {
  const [drawn, setDrawn] = useState(open)
  // The count last painted, not the count this effect has seen. A second run
  // of the effect for the same commit — StrictMode, in development — must
  // still wait the frame, so the count moves only when the frame does.
  const painted = useRef(columns)
  useLayoutEffect(() => {
    if (painted.current === columns) {
      setDrawn(open)
      return
    }
    const id = requestAnimationFrame(() => {
      painted.current = columns
      setDrawn(open)
    })
    return () => cancelAnimationFrame(id)
  }, [open, columns])
  return drawn
}
