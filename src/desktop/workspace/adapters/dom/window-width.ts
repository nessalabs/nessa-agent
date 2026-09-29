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
 * Called as the change is laid out, before the frame paints: the fold lands in
 * the frame of the resize, and nothing is left waiting for a later effect for
 * a resize observer's synchronous render to flush out of turn.
 */
export function useFitOnResize(
  windowWidth: number,
  columns: number,
  fit: (windowWidth: number) => void,
) {
  const latest = useRef(fit)
  latest.current = fit
  useLayoutEffect(() => {
    latest.current(windowWidth)
  }, [windowWidth, columns])
}
