import * as React from "react"

import { placeWindowControls } from "../../host/window"

/**
 * Keeps the window's own controls on the box, wherever the box is.
 *
 * Setup covers the screen, so AppKit would wear the close button in the
 * display's corner — stranded in the dimmed area, nowhere near the surface it
 * closes. The page is the only thing that knows where that surface is, so it
 * measures it and says. It re-measures on a resize because the box is sized
 * against the viewport.
 */
export function useWindowControls(box: React.RefObject<HTMLElement | null>) {
  React.useEffect(() => {
    function place() {
      const element = box.current
      if (!element) return
      const { left, top } = element.getBoundingClientRect()
      void placeWindowControls(left, top)
    }
    place()
    window.addEventListener("resize", place)
    return () => window.removeEventListener("resize", place)
  }, [box])
}
