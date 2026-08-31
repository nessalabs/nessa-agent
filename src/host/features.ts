/**
 * What the shell needs from the host OS. One object is injected for the
 * running host (`resolveHost`); `App` and CSS read these fields instead of
 * branching on `macos` / `linux`.
 */
export type HostKind = "macos" | "linux" | "browser" | "other"

/**
 * How the frosted surface is painted.
 *
 * `native` is an `NSVisualEffectView` behind the webview (macOS Tauri). A CSS
 * `backdrop-filter` on that window smears the behind-window content and
 * freezes when unfocused, so the shell must not add one.
 *
 * `css` is `backdrop-filter` on the panel itself (Linux, a browser preview,
 * everywhere else).
 */
export type FrostKind = "native" | "css"

/**
 * How the webview composites the page.
 *
 * `layer` is a normal compositor (macOS WKWebView, a browser). Mount springs
 * and per-letter fades are safe.
 *
 * `layout` is WebKitGTK in a transparent window: transform/filter layers
 * paint from the panel origin and vacated tiles stay on screen. The
 * transcript stays in layout and the panel remounts the log when a turn
 * lands so those tiles are dropped.
 */
export type CompositorKind = "layer" | "layout"

export interface HostFeatures {
  readonly kind: HostKind
  readonly frost: FrostKind
  readonly compositor: CompositorKind
  /**
   * Class for the west resize handle. Linux's handle *is* the resize (12px)
   * and stops above the composer. It must not carry a z-index: a stacking
   * context on the panel makes WebKitGTK fill an opaque white layer from
   * the window's left edge, squaring off the pill's left cap. macOS only
   * covers the band inside the system's own grab zone.
   */
  readonly westHandleClass: string
  /**
   * Whether the page captures the pointer on the west handle. Linux must not:
   * capture holds the button on the webview and GTK never sees the press that
   * starts `_NET_WM_MOVERESIZE`.
   */
  readonly capturePointerOnWestHandle: boolean
  /** Mount springs, per-letter fades, filter transitions, WAAPI typing dots. */
  readonly animateMount: boolean
  /** Reveal the assistant turn a letter at a time. */
  readonly streamText: boolean
  /** Idle empty-state avatar. Safe on every host; motion follows animateMount. */
  readonly emptyState: boolean
}

export const WEST_HANDLE_NARROW = "nessa-west-handle nessa-west-handle-narrow"
export const WEST_HANDLE_WIDE = "nessa-west-handle nessa-west-handle-wide"
