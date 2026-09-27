/**
 * The window's width, followed as it resizes. The workspace draws its side
 * columns from it (`model/window-fit.ts` decides how wide), and folds them
 * when the window grows too narrow for the panes beside them.
 */
import { useEffect, useRef, useSyncExternalStore } from "react"

function subscribe(listener: () => void) {
  window.addEventListener("resize", listener)
  return () => window.removeEventListener("resize", listener)
}

export function useWindowWidth(): number {
  return useSyncExternalStore(subscribe, () => window.innerWidth)
}

/**
 * Calls `fit` with the window's width when the window is resized (and once
 * on mount) — only then, so a column opened by hand in a small window stays
 * open until the window changes.
 */
export function useFitOnResize(windowWidth: number, fit: (windowWidth: number) => void) {
  const latest = useRef(fit)
  latest.current = fit
  useEffect(() => {
    latest.current(windowWidth)
  }, [windowWidth])
}
