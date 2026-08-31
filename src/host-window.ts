/**
 * The desktop window controls, guarded so the same UI also runs in a plain
 * browser (`pnpm dev`) where there is no Tauri host to talk to.
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window

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
  return listen("nessa://toggle-surface", () => handler())
}

/**
 * Subscribes to the panel being summoned, so the composer can take the caret.
 */
export async function onFocusComposer(handler: () => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen("nessa://focus-composer", () => handler())
}

/** The host window's size, in CSS pixels. */
export interface HostSize {
  width: number
  height: number
}

/**
 * Subscribes to the window changing size, whoever is driving it. Returns a
 * promise for the unsubscribe, and a no-op outside Tauri.
 *
 * The size is carried because the page can no longer read it off its own
 * viewport. The webview is deliberately larger than the window and pinned to
 * its bottom right corner, so that a resize never moves the page's viewport and
 * so cannot displace anything already drawn — see `src-tauri/src/viewport.rs`
 * for why that is the only place the displacement can be fixed. `innerHeight`
 * therefore describes the stage the panel stands on, not the panel.
 */
export async function onWindowResize(handler: (size: HostSize) => void) {
  if (!inTauri) return () => undefined
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  return getCurrentWindow().onResized(({ payload }) => handler(logical(payload)))
}

/**
 * The window's size now, for the first paint. The panel is summoned hidden and
 * sized before it is shown, so the opening size normally arrives as an event —
 * but a reload mid-session has no event coming and would otherwise draw the
 * panel the size of the whole stage until the reader next dragged an edge.
 */
export async function windowSize(): Promise<HostSize | null> {
  if (!inTauri) return null
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  return logical(await getCurrentWindow().innerSize())
}

/** Window sizes arrive in device pixels; CSS is written in logical ones. */
function logical(size: { width: number; height: number }): HostSize {
  const scale = window.devicePixelRatio || 1
  return { width: size.width / scale, height: size.height / scale }
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
    listen("nessa://resize-started", () => handler(true)),
    listen("nessa://resize-ended", () => handler(false)),
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
