import { maskRustNonCode, normalizedPath } from "./rust-boundaries.mjs"

/**
 * The setup window is positioned by `Host::place_overlay` and by nothing else.
 *
 * A position asked of the window builder is not a second opinion, it is the
 * last word: on macOS `setFrameTopLeftPoint` may not be called off the main
 * thread, so tao queues it, and the queued block lands a turn of the event loop
 * *after* `place_overlay` — which runs during `setup`, before the loop turns at
 * all. A screen-covering overlay was pulled back to where the unplaced 960x640
 * window would have been centred, leaving the menu bar and a strip of desktop
 * undimmed beside it and the page's content visibly off centre.
 *
 * Only the builder call is forbidden. `place_overlay` itself centres the window
 * on hosts that cannot cover the screen, which is the point.
 */
export function overlayPlacementViolations(path, source) {
  if (normalizedPath(path) !== "src-tauri/src/panel.rs") return []

  const code = maskRustNonCode(source)
  const builder = code.match(/fn\s+build_setup_window\b[\s\S]*?\n\}/)?.[0]
  if (!builder) {
    return [
      "panel.rs no longer declares build_setup_window; move the overlay placement check with it",
    ]
  }

  const failures = []
  if (/\.\s*center\s*\(/.test(builder)) {
    failures.push(
      "the setup window builder must not ask to be centred; Host::place_overlay positions it, and a builder position is re-applied after that call",
    )
  }
  if (/\.\s*position\s*\(/.test(builder)) {
    failures.push(
      "the setup window builder must not ask for a position; Host::place_overlay positions it, and a builder position is re-applied after that call",
    )
  }
  return failures
}
