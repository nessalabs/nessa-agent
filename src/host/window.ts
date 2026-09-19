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
  updateAvailable: "nessa://update-available",
  updateProgress: "nessa://update-progress",
  updateFailed: "nessa://update-failed",
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

/**
 * A published release newer than the running build. Matches `updater::Release`.
 *
 * `notes` is what the release manifest published, or `null` when it published
 * none — which is what ships first, since nothing fills that field yet.
 */
export interface Release {
  from: string
  version: string
  notes: string | null
}

/** How far the download has got. Matches `updater::Downloaded`. */
export interface Downloaded {
  downloaded: number
  /** What the server declared, or `null` when it declared nothing. */
  total: number | null
}

/**
 * The release the host has already found, for a panel that has just mounted.
 *
 * The check runs once at launch and can finish before this page exists — or
 * long before, on a launch where the panel is never opened — so the event alone
 * would lose it. The host keeps the announcement and answers this with it.
 */
export async function availableUpdate(): Promise<Release | null> {
  if (!inTauri) return null
  const { invoke } = await import("@tauri-apps/api/core")
  return invoke<Release | null>("available_update")
}

/** Subscribes to a check finding a newer release. */
export async function onUpdateAvailable(handler: (release: Release) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<Release>(HOST_EVENTS.updateAvailable, ({ payload }) => handler(payload))
}

/** Subscribes to the progress of the download this panel asked for. */
export async function onUpdateProgress(handler: (progress: Downloaded) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<Downloaded>(HOST_EVENTS.updateProgress, ({ payload }) => handler(payload))
}

/**
 * Subscribes to that download being refused, with the host's reason.
 *
 * The one update failure that reaches the screen: a check nobody asked for
 * stays quiet, but an install somebody asked for owes them an answer.
 */
export async function onUpdateFailed(handler: (reason: string) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<string>(HOST_EVENTS.updateFailed, ({ payload }) => handler(payload))
}

/**
 * Downloads the announced update and restarts onto it.
 *
 * The host owns all of it — the endpoint, the signature, the install — so this
 * only asks. Progress and refusal come back as events; success comes back as
 * the app restarting.
 */
export async function installUpdate(): Promise<void> {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("install_update")
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
 * Outcomes rather than a rejection, because the caller has to act on all of
 * them and only one is a panel fault: a browser has no second window, and a
 * setup window that is still on screen — whether because the panel never came
 * up or because the window itself would not go — must not sit there empty.
 *
 * The last two are deliberately distinct. They put different things on screen
 * and offer different ways out, and collapsing them told people the panel was
 * unavailable while it stood open behind the sentence saying so.
 */
export type SetupHandoff =
  /** The panel is up and this window is closing. */
  | { outcome: "handed-over" }
  /** No second window exists; the caller renders the panel in place. */
  | { outcome: "no-native-host" }
  /** The panel did not come up. This window is still on screen. */
  | { outcome: "panel-unavailable"; cause: unknown }
  /** The panel is up; this window is the only thing that did not go. */
  | { outcome: "setup-close-failed"; panelShown: true; cause: unknown }

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
 * The recovery screen's way out, and only that. The handoff closes this window
 * on the host, in order and after the completion write (`finishSetupWindow`);
 * this is what the setup surface offers when the handoff left the window on
 * screen and it would otherwise sit there for good.
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

/** `panel::SetupHandoff`, as the host serializes it. */
interface NativeSetupHandoff {
  setupClosed: boolean
  closeError: string | null
  recordError: string | null
}

/**
 * Hand off from setup to the panel.
 *
 * One call, because the sequence behind it — show the panel, record that setup
 * finished, close this window — has to survive this window. Running it from
 * here meant the completion write was issued by a webview that had already
 * awaited its own destruction, so a teardown that got there first left
 * `onboarding.completed` unset on a machine somebody had just set up. The order
 * now lives in `panel::finish_setup`, in the process that outlives the window.
 *
 * `completed` is how setup ended: a finish, or somebody leaving. Only a finish
 * is written off for good, which is the host's rule to apply — this carries the
 * fact, not the decision. `agent` is what was chosen, recorded with it, because
 * the panel is a different window and cannot be handed anything this one knows.
 *
 * Outside Tauri there is no second window, so this is a no-op and the caller
 * simply carries on rendering the panel in place.
 *
 * In the ordinary case this promise never settles: the window it was called
 * from is gone before the answer gets back. Everything that had to happen has
 * happened by then.
 */
export async function finishSetupWindow(
  completed: boolean,
  agent?: string,
): Promise<SetupHandoff> {
  if (!inTauri) return { outcome: "no-native-host" }
  const { invoke } = await import("@tauri-apps/api/core")
  let handoff: NativeSetupHandoff
  try {
    handoff = await invoke<NativeSetupHandoff>("finish_setup", {
      completed,
      agent: agent ?? null,
    })
  } catch (cause) {
    // The one step that abandons the handoff. Nothing was written and this
    // window is still on screen, which is what the surface has to say.
    return { outcome: "panel-unavailable", cause }
  }
  if (handoff.recordError) {
    // Survivable, and already logged on the host's side. It costs the next
    // launch's straight start, not this one's panel.
    console.warn("[nessa] could not record that setup finished", handoff.recordError)
  }
  // The panel is up. A window that will not close is not a panel failure and is
  // no longer reported as one — the surface says what actually happened and
  // offers a close rather than another handoff.
  if (!handoff.setupClosed) {
    return { outcome: "setup-close-failed", panelShown: true, cause: handoff.closeError }
  }
  return { outcome: "handed-over" }
}

/**
 * Show the setup window, once its page has rendered.
 *
 * It is created hidden: a window is on screen the moment it exists, and a
 * webview has painted nothing the moment it is created, so a window visible
 * from the start shows whatever the window server has for it until the first
 * frame lands — a flash at exactly the point the opening begins from darkness.
 *
 * Rendered, not painted. A hidden window is never drawn, so its page cannot
 * wait for a frame to arrive before asking for this — see
 * `onboarding/ui/reveal-on-first-render.ts`, which is where that waiting stopped.
 */
export async function revealSetupWindow() {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("reveal_setup_window")
}

/**
 * The agent first-run setup chose, as the host recorded it.
 *
 * Read rather than remembered: setup runs in its own window, which is gone by
 * the time the panel needs the answer. Undefined where nobody has chosen — a
 * first run still in progress, a setup that was left, or a browser with no host
 * to ask — and the gateway then starts conversations on its own default rather
 * than being told an agent nobody picked.
 */
export async function loadChosenAgent(): Promise<string | undefined> {
  if (!inTauri) return undefined
  try {
    // The import is inside the try with the call it makes. Loading the host
    // module is a fetch like any other and can fail on its own; left outside,
    // that failure would come back as a rejection from a function whose whole
    // contract is that it answers.
    const { invoke } = await import("@tauri-apps/api/core")
    return (await invoke<string | null>("chosen_agent")) ?? undefined
  } catch (cause) {
    // Survivable: the conversation starts on the gateway's default instead of
    // on the recorded choice, which is a worse answer and not a broken panel.
    console.warn("[nessa] could not read the agent setup chose", cause)
    return undefined
  }
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
