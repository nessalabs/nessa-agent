/**
 * The window's width, followed as it resizes: what the side columns — the
 * workspace's, Settings' sidebar — are drawn and folded by.
 */
import { useSyncExternalStore } from "react"

function subscribe(listener: () => void) {
  window.addEventListener("resize", listener)
  return () => window.removeEventListener("resize", listener)
}

export function useWindowWidth(): number {
  return useSyncExternalStore(subscribe, () => window.innerWidth)
}
