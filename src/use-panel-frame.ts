import * as React from "react"

import { onWindowResize, windowSize } from "./host-window"

/**
 * Tells the page how big the host window is.
 *
 * The webview is deliberately larger than the window and pinned to its bottom
 * right corner, so that a resize never moves the page's viewport: a `WKWebView`
 * hangs the last frame the web process committed off its own top left corner,
 * and moving that corner is what dragged the composer around by 90pt during a
 * resize. `src-tauri/src/viewport.rs` has the full account.
 *
 * The cost of holding the viewport still is that it no longer describes the
 * window, so the size is handed to CSS instead and the panel is drawn that big
 * against the bottom right of the stage. Both custom properties fall back to
 * `100%`, which is the right answer in a plain browser (`pnpm dev`) where the
 * viewport *is* the window.
 *
 * The properties are written straight to the document element rather than held
 * in state. A resize drag delivers a size event per mouse event, and
 * re-rendering the conversation tree at that rate is both wasteful and, during
 * a live resize, precisely the work that makes the panel's edge trail the
 * window's.
 */
export function usePanelFrame() {
  React.useEffect(() => {
    const root = document.documentElement
    let stale = false

    const publish = (size: { width: number; height: number } | null) => {
      if (!size || stale) return
      root.style.setProperty("--nessa-window-width", `${size.width}px`)
      root.style.setProperty("--nessa-window-height", `${size.height}px`)
    }

    void windowSize().then(publish)
    const subscription = onWindowResize(publish)

    return () => {
      // The size is read asynchronously, so an unmount can land first; without
      // this the resolved size would be written to a document the panel no
      // longer owns.
      stale = true
      void subscription.then((unlisten) => unlisten())
    }
  }, [])
}
