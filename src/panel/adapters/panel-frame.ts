import * as React from "react"

import { onWindowResize, windowSize } from "../../host"

/**
 * Tells the page how big the host window is.
 *
 * The webview is deliberately larger than the window and pinned to its bottom
 * right corner, so that a resize never moves the page's viewport: a `WKWebView`
 * hangs the last frame the web process committed off its own top left corner,
 * and moving that corner is what dragged the composer around by 90pt during a
 * resize. `src-tauri/src/platform/macos/viewport.rs` has the full account.
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

    // A size that is not a real length would be written as an invalid custom
    // property, which CSS resolves not to the `100%` fallback but to `auto` —
    // and an `auto` panel collapses to the height of the composer, because the
    // transcript's `flex-1` has nothing definite to divide. Refusing the write
    // keeps the fallback reachable.
    const publish = (size: { width: number; height: number } | null) => {
      if (stale || !size) return
      if (!Number.isFinite(size.width) || !Number.isFinite(size.height)) return
      if (size.width <= 0 || size.height <= 0) return
      root.style.setProperty("--nessa-window-width", `${size.width}px`)
      root.style.setProperty("--nessa-window-height", `${size.height}px`)
    }

    void windowSize().then(publish, (error) => {
      // Silence here would leave the panel drawn the size of the whole stage,
      // which is a great deal harder to recognise than a line in the log.
      console.error("[nessa] could not read the panel's size", error)
    })
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
