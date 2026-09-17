/**
 * The desktop window controls, guarded so the same UI also runs in a plain
 * browser (`pnpm dev`) where there is no Tauri host to talk to.
 *
 * Event names and the size payload are the host/shell contract. They are
 * declared here and again in `src-tauri/src/host.rs`; a Rust test fails if a
 * name on that side is missing from this file.
 */
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window

const HOST_EVENTS = {
  toggleSurface: "nessa://toggle-surface",
  summoned: "nessa://summoned",
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
 * Subscribes to the global summon accelerator, reporting whether the panel is
 * now showing.
 *
 * The host registers that accelerator with the system, so the press is taken
 * before any window sees a key. A surface that teaches the shortcut cannot
 * learn it was pressed any other way.
 */
export async function onSummoned(handler: (showing: boolean) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<boolean>(HOST_EVENTS.summoned, ({ payload }) => handler(payload))
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
 * so cannot displace anything already drawn — see `src-tauri/src/platform/`.
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
 * Drops vacated compositor tiles. WebKitGTK keeps the previous frame of a
 * bubble that has changed y; macOS and the browser preview do not need this.
 */
export async function flushCompositor() {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("flush_compositor")
}

/** Stage-scoped shortcuts cache (or null outside Tauri — use bundled defaults). */
export async function loadShortcuts(): Promise<
  import("@nessa/client").ShortcutsDocument | null
> {
  if (!inTauri) return null
  const { invoke } = await import("@tauri-apps/api/core")
  return invoke("load_shortcuts")
}

/** Persist configured shortcuts and re-register global summon on the host. */
export async function applyShortcuts(
  document: import("@nessa/client").ShortcutsDocument,
): Promise<void> {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("apply_shortcuts", { document })
}

/**
 * Subscribes to the window being live-resized — held by its frame, as opposed
 * to resized programmatically.
 *
 * macOS runs that gesture itself and tells the webview nothing about it, so
 * the host forwards AppKit's own notifications (see `platform/macos/live_resize.rs`); there
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

/** Whether this page runs inside the trusted desktop host. */
/**
 * Which surface this window was opened to paint.
 *
 * The desktop host opens setup in its own window pointed at the same bundle
 * with `?surface=setup`, so the query is the window's identity as far as the
 * shell is concerned. A plain browser has no second window and always paints
 * the panel.
 */
export function windowSurface(): "panel" | "setup" | "setup-dim" {
  if (typeof window === "undefined") return "panel"
  const surface = new URLSearchParams(window.location.search).get("surface")
  if (surface === "setup" || surface === "setup-dim") return surface
  return "panel"
}

/**
 * Hand off from setup to the panel: show the panel window, then close this one.
 *
 * Outside Tauri there is no second window, so this is a no-op and the caller
 * simply carries on rendering the panel in place.
 */
export async function finishSetupWindow() {
  if (!inTauri) return
  // Through the host, not `show()` on the window: the panel is anchored to an
  // edge of the work area and its webview fitted to the window, and a page
  // cannot do either. Showing it from here left it wherever the window system
  // happened to put it.
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("summon_panel")
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  await getCurrentWindow().close()
}

/**
 * Open the setup window, once the opening has produced something for it to be.
 *
 * It is built at this moment rather than waiting hidden, so that its frame is
 * not on screen during the opening and its own animations start when it
 * appears rather than having run out while nobody could see them.
 */
export async function openSetupWindow() {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("open_setup_window")
}

/**
 * Close the dim once it has nothing left to do.
 *
 * The host has already hidden it by the time this runs, so this is
 * housekeeping: nothing visible depends on it, which is the point — a window
 * covering the screen must never be the only thing that can remove it.
 */
export async function closeSetupDim() {
  if (!inTauri) return
  const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow")
  const dim = await WebviewWindow.getByLabel("setup-dim")
  await dim?.close()
}

export function hasNativeHost(): boolean {
  return inTauri
}

/** Load only the bundled chat credential from native private storage. */
export async function loadAssignedSurfaceCredential(stage: string): Promise<string> {
  if (!inTauri) throw new Error("A native host is required for local credential storage")
  const { invoke } = await import("@tauri-apps/api/core")
  try {
    return await invoke<string>("load_surface_credential", { stage })
  } catch (error) {
    // Tauri serializes command failures rather than constructing JS Errors.
    // Preserve only the native boundary's safe message, never arbitrary payloads.
    if (error instanceof Error) throw error
    const message =
      typeof error === "string"
        ? error
        : error && typeof error === "object" && "message" in error
          ? error.message
          : undefined
    throw new Error(
      typeof message === "string" && message.trim()
        ? message
        : "Could not load the desktop gateway credential.",
      { cause: error },
    )
  }
}
