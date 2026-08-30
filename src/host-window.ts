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

/**
 * Starts a live resize from the panel's left edge.
 *
 * An undecorated macOS window has no system resize border — `resizable: true`
 * alone gives nothing to grab — so the edge is a real element the window
 * drags from. West only: the panel is pinned to the right of the screen and
 * spans the work area's height, so width is the one dimension it owns.
 */
export async function startResizeFromLeftEdge() {
  if (!inTauri) return
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  await getCurrentWindow().startResizeDragging("West")
}

export { inTauri }
