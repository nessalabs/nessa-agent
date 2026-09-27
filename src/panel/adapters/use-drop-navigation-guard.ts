import * as React from "react"

/** The panel accepts content drops; a webview must never navigate to their URL. */
export function useDropNavigationGuard() {
  React.useEffect(() => {
    const preventNavigation = (event: DragEvent) => event.preventDefault()
    window.addEventListener("dragover", preventNavigation)
    window.addEventListener("drop", preventNavigation)
    return () => {
      window.removeEventListener("dragover", preventNavigation)
      window.removeEventListener("drop", preventNavigation)
    }
  }, [])
}
