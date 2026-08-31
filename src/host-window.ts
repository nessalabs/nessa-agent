/**
 * The desktop window controls, guarded so the same UI also runs in a plain
 * browser (`pnpm dev`) where there is no Tauri host to talk to.
 *
 * Event names and the size payload are the host/shell contract. They are
 * declared here and again in `src-tauri/src/host.rs`; a Rust test fails if a
 * name on that side is missing from this file.
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window

/**
 * Which host is drawing the panel. Decides whether frost is native (macOS
 * Tauri) or CSS (Linux, a browser, everywhere else). The macOS CSS
 * `backdrop-filter` path is the one that smears on an undecorated window;
 * everywhere else it is the frost.
 */
export const hostKind: "macos" | "linux" | "browser" | "other" = !inTauri
  ? "browser"
  : navigator.userAgent.includes("Mac")
    ? "macos"
    : navigator.userAgent.includes("Linux")
      ? "linux"
      : "other"

export const HOST_EVENTS = {
  toggleSurface: "nessa://toggle-surface",
  focusComposer: "nessa://focus-composer",
  panelSized: "nessa://panel-sized",
  resizeStarted: "nessa://resize-started",
  resizeEnded: "nessa://resize-ended",
} as const

/**
 * The frosted surface is a native window effect, so the clear surface has to
 * turn it off in the host as well as in CSS.
 */
export async function setFrosted(frosted: boolean) {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("set_frosted", { frosted })
}

/**
 * Subscribes to the tray menu's surface request. Returns a promise for the
 * unsubscribe, and a no-op outside Tauri where there is no tray.
 */
export async function onToggleSurface(handler: () => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen(HOST_EVENTS.toggleSurface, () => handler())
}

/**
 * Subscribes to the panel being summoned, so the composer can take the caret.
 */
export async function onFocusComposer(handler: () => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen(HOST_EVENTS.focusComposer, () => handler())
}

/** The host window's size, in CSS pixels. Matches `host::PanelSize`. */
export interface PanelSize {
  width: number
  height: number
}

/**
 * Subscribes to the window changing size, whoever is driving it. Returns a
 * promise for the unsubscribe, and a no-op outside Tauri.
 *
 * The size is carried, and it comes from the host rather than from Tauri's own
 * window APIs. The webview is deliberately larger than the window and pinned to
 * its bottom right corner, so that a resize never moves the page's viewport and
 * so cannot displace anything already drawn — see `src-tauri/src/viewport.rs`.
 * Tauri reads a window's inner size off that same view, so with the view
 * detached from the window `innerSize()` answers with the stage, agreeing with
 * `innerHeight` and with nothing the reader can see. The host reads AppKit's
 * content rect instead, which a fixed webview cannot falsify, and sends it.
 */
export async function onWindowResize(handler: (size: PanelSize) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<PanelSize>(HOST_EVENTS.panelSized, ({ payload }) => handler(payload))
}

/**
 * The window's size now, for a page with no size event coming. The host sends
 * one as it places the panel, which a webview that reloaded mid-session — a
 * devtools reload — will have missed.
 */
export async function windowSize(): Promise<PanelSize | null> {
  if (!inTauri) return null
  const { invoke } = await import("@tauri-apps/api/core")
  return invoke<PanelSize>("panel_size")
}

/**
 * Subscribes to the window being live-resized — held by its frame, as opposed
 * to resized programmatically.
 *
 * macOS runs that gesture itself and tells the webview nothing about it, so
 * the host forwards AppKit's own notifications (see `live_resize.rs`); there
 * is no reading it off the page's own events. Outside Tauri there is no
 * window frame to hold, so the handler is simply never called.
 */
export async function onLiveResize(handler: (active: boolean) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  const unlisten = await Promise.all([
    listen(HOST_EVENTS.resizeStarted, () => handler(true)),
    listen(HOST_EVENTS.resizeEnded, () => handler(false)),
  ])
  return () => {
    for (const stop of unlisten) stop()
  }
}

/**
 * Starts a live resize from the panel's left edge.
 *
 * West only: the panel is pinned to the right of the screen and spans the work
 * area's height, so width is the one dimension it owns.
 *
 * In practice macOS claims the frame first and resizes the window without ever
 * telling the webview, so this runs only when the pointer lands inside the
 * handle but outside the system's own grab zone. It is kept as the fallback
 * for that band and for hosts with no system resize border of their own; the
 * border glow deliberately does not depend on it firing (see `useEdgeReveal`).
 */
export async function startResizeFromLeftEdge() {
  if (!inTauri) return
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  await getCurrentWindow().startResizeDragging("West")
}

export { inTauri }
