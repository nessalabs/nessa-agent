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

/**
 * Which surface this window was opened to paint.
 *
 * The desktop host opens setup in its own window pointed at the same bundle
 * with `?surface=setup`, so the query is the window's identity as far as the
 * shell is concerned. A plain browser has no second window and always paints
 * the panel.
 */
export function windowSurface(): "panel" | "setup" {
  if (typeof window === "undefined") return "panel"
  return new URLSearchParams(window.location.search).get("surface") === "setup"
    ? "setup"
    : "panel"
}

/**
 * What handing off from setup to the panel actually did.
 *
 * Three outcomes rather than a rejection, because the caller has to act on all
 * three and two of them are not faults: a browser has no second window, and a
 * panel that could not be summoned still leaves a setup window that must not
 * sit there empty.
 */
export type SetupHandoff =
  /** The panel is up and this window is closing. */
  | { outcome: "handed-over" }
  /** No second window exists; the caller renders the panel in place. */
  | { outcome: "no-native-host" }
  /** The panel did not come up. This window is still on screen. */
  | { outcome: "panel-unavailable"; cause: unknown }

/**
 * What closing this window did.
 *
 * Three outcomes rather than a rejection, for the same reason the handoff has
 * them: a browser tab has no window of its own to close, and a close that the
 * window system refuses is something the surface has to keep a screen up about
 * rather than crash on.
 */
export type SetupWindowClose =
  /** The window is closing. */
  | { outcome: "closed" }
  /** There is no host window here; nothing was closed. */
  | { outcome: "no-native-host" }
  /** The window is still on screen, and why. */
  | { outcome: "close-failed"; cause: unknown }

/**
 * Close the window this page is painted in.
 *
 * The direct way out, with no panel in it: the handoff below uses it after the
 * panel is up, and the setup surface offers it on its own when the handoff
 * could not be completed and the window would otherwise sit there for good.
 */
export async function closeSetupWindow(): Promise<SetupWindowClose> {
  if (!inTauri) return { outcome: "no-native-host" }
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  try {
    await getCurrentWindow().close()
  } catch (cause) {
    return { outcome: "close-failed", cause }
  }
  return { outcome: "closed" }
}

/**
 * Hand off from setup to the panel: show the panel window, then close this one.
 *
 * Outside Tauri there is no second window, so this is a no-op and the caller
 * simply carries on rendering the panel in place.
 *
 * The close is not conditional on the summon. A failed summon used to abandon
 * the handoff half way, leaving a setup window that had already stopped
 * painting setup — and nothing said so. If the panel cannot be summoned the
 * failure is returned rather than thrown, so the surface can say so instead of
 * showing an empty window.
 */
export async function finishSetupWindow(): Promise<SetupHandoff> {
  if (!inTauri) return { outcome: "no-native-host" }
  // Through the host, not `show()` on the window: the panel is anchored to an
  // edge of the work area and its webview fitted to the window, and a page
  // cannot do either. Showing it from here left it wherever the window system
  // happened to put it.
  const { invoke } = await import("@tauri-apps/api/core")
  try {
    await invoke("summon_panel")
  } catch (cause) {
    return { outcome: "panel-unavailable", cause }
  }
  // The panel is up, so a window that will not close is not a panel failure and
  // is not reported as one: the cause travels out as it did before, and the
  // surface that catches it offers a close of its own.
  const closed = await closeSetupWindow()
  if (closed.outcome === "close-failed") throw closed.cause
  return { outcome: "handed-over" }
}

/**
 * Record that first-run setup finished, so the next launch opens the panel
 * instead of setup.
 *
 * The host keeps this in its settings file; there is nothing to write to
 * outside Tauri, where a reload starts over anyway, so this is an explicit
 * no-op rather than a pretend success. It rejects when the host could not
 * write — which costs the next launch's straight start, not this one's handoff,
 * so the caller logs it and carries on.
 */
export async function recordSetupComplete(): Promise<void> {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("complete_onboarding")
}

/**
 * Show the setup window, once its page has a frame to show.
 *
 * It is created hidden: a window is on screen the moment it exists, and a
 * webview has painted nothing the moment it is created, so a window visible
 * from the start shows whatever the window server has for it until the first
 * frame lands — a flash at exactly the point the opening begins from darkness.
 */
export async function revealSetupWindow() {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("reveal_setup_window")
}

/** Whether this page runs inside the trusted desktop host. */
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
