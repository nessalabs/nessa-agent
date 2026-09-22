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
  linkNotOpened: "nessa://link-not-opened",
  attachmentDropped: "nessa://attachment-dropped",
  attachmentDragging: "nessa://attachment-dragging",
  attachmentReadying: "nessa://attachment-readying",
  attachmentBatch: "nessa://attachment-batch",
} as const

export interface WebviewConsoleEntry {
  level: "warn" | "error"
  message: string
  source: string
}

/** Mirrors a development webview warning or error to the host terminal. */
export async function forwardWebviewConsole(entry: WebviewConsoleEntry): Promise<void> {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("plugin:dev-console|forward_webview_console", { entry })
}

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

/** One file the person chose in the host's picker. Matches `attachments::ChosenFile`. */
export interface ChosenFile {
  /** The absolute path, as the operating system spells it. */
  path: string
  /** The final component of that path. */
  name: string
  /** The file's length in bytes. */
  size: number
  /**
   * What the operating system says this file is: lowercase, no parameters, in
   * the same shape a browser puts in `File.type`.
   *
   * It goes exactly where a dropped or pasted file's `type` goes, which is the
   * point of it being here — what becomes of an attachment is decided by its
   * type and never by the gesture that attached it, so a file picked through
   * the host and the same file dropped on the panel must take the same route.
   * macOS answers from the path's Uniform Type Identifier and Linux from
   * shared-mime-info, so a format this app's extension table has never heard of
   * (`.ico`, `.svgz`, `.jp2`, `.xbm`) is still recognised as an image.
   *
   * Empty where the platform has no answer — Windows always, and macOS for a
   * type it knows and has no MIME name for. The host never guesses one from the
   * extension: that is the caller's fallback, and it must not also be the
   * host's, or the two could disagree.
   */
  mimeType: string
  /**
   * The one-shot ticket {@link readAttachmentBytes} reads this file with.
   *
   * Opaque, unguessable, and good exactly once. It is what authorises a read —
   * `path` is not, and has not been since the host stopped opening whatever
   * path the page named. Hand it back unchanged and do not keep it after it has
   * been used: a second presentation rejects with `ticket-already-used`.
   */
  ticket: string
}

/**
 * Ask the host for the paths of files to attach.
 *
 * `null` outside Tauri, where there is no picker and no filesystem to name: the
 * caller falls back to the browser's own file input, which can read a file's
 * bytes but never learn where it came from.
 *
 * An empty array is a cancellation — the picker opened and nothing was chosen —
 * and is not a failure. A file the host cannot carry faithfully rejects with
 * `attachments::FileNotAttached` rather than arriving renamed or quietly
 * missing from the array: a path that is not valid UTF-8 (`path-not-text`),
 * something that is not an ordinary file such as a directory or a named pipe
 * (`not-a-regular-file`), or a disk that never answered (`filesystem-stalled`).
 * One unusable file refuses the whole selection.
 *
 * Every file comes back with a {@link ChosenFile.ticket}. Keep it: it is the
 * only way to read that file's bytes, and it is good once.
 */
export async function chooseAttachmentFiles(): Promise<ChosenFile[] | null> {
  if (!inTauri) return null
  const { invoke } = await import("@tauri-apps/api/core")
  return invoke<ChosenFile[]>("choose_attachment_files")
}

/**
 * What a drag put on the panel, as the host describes it.
 *
 * A drag carries one of the two: `files` for a drag of files, `text` for a
 * drag of anything else. `refused` says why there is neither.
 */
export interface DroppedOnPanel {
  /**
   * Which drop this is — the same name {@link onAttachmentDragging} reported
   * the moment it landed, and the only link back to the conversation the
   * gesture happened over. The files can be three quarters of a minute behind
   * the drop, by which time the open tab may be a different one.
   */
  batch: string
  /**
   * Files, in the order the operating system gave them, each already described
   * and ticketed exactly as {@link chooseAttachmentFiles} describes a picked
   * one — so a dragged file and a picked file are the same thing by the time
   * the panel sees them, which is the whole product rule.
   */
  files: ChosenFile[]
  /**
   * What a drag of text or of a web-page image carried, under the names
   * `DataTransfer` uses, so the panel's existing readers take it unchanged.
   */
  text: { plain: string; uriList: string; html: string }
  /**
   * Why nothing was attached, in `attachments::FileNotAttached`'s own shape. A
   * drop is refused whole or not at all.
   *
   * `unknown` on purpose: the panel reads it with `pickerRefusal`, which is
   * the one place that knows the host's reason names, and which takes whatever
   * arrives rather than trusting a second copy of that vocabulary to have
   * stayed in step. The same value reaches it here as a payload and from the
   * picker as a rejection.
   */
  refused: unknown
}

/**
 * Be told when something is dropped on the panel.
 *
 * **The page no longer receives drops of its own.** `dragDropEnabled` is on,
 * which is the only way a dropped file's path can be known at all, and it costs
 * the webview every HTML5 drag event — not only the ones carrying files. So
 * this is the sole source of drops, and everything that used to arrive through
 * `DataTransfer` arrives here instead.
 *
 * **The page is never asked to name a path.** The host describes the dropped
 * paths itself and mints their tickets, so a dropped file reaches the page in
 * the same shape a picked one does and the page still cannot ask the host to
 * open something nobody dropped.
 */
export async function onAttachmentDropped(
  handler: (dropped: DroppedOnPanel) => void,
): Promise<() => void> {
  if (!inTauri) return () => {}
  const { listen } = await import("@tauri-apps/api/event")
  return listen<DroppedOnPanel>(HOST_EVENTS.attachmentDropped, ({ payload }) =>
    handler(payload),
  )
}

/**
 * Be told while a drag is over the panel, so the drop target can be drawn.
 *
 * The page used to know this from its own `dragenter` and `dragleave`. It
 * receives neither now, and without the host saying so the panel would accept
 * a dropped file perfectly well while giving no sign beforehand that it would.
 */
/**
 * Be told while a chosen file is being made readable.
 *
 * A file a cloud service is keeping is not on this disk, and the host asks for
 * it rather than refusing — which can take up to forty-five seconds. The answer
 * arrives at the end of that, so without this the panel would say nothing at
 * all while it happened, and silence reads as broken.
 *
 * `id` is the identity and `name` is only a label. Two folders can each hold a
 * `report.pdf`, and a panel keyed on the name kept one entry for both: the
 * first to settle took the other's tile away and let the draft be sent while
 * the second was still being fetched. The identity is minted by the host,
 * where the work is, and begins with the batch the drop announced — so a tile
 * belongs to the draft the gesture landed on rather than to whichever tab is
 * open when the host finally speaks.
 *
 * `readying` false means that file has stopped waiting — it arrived, or it will
 * not — and the tile goes either way.
 *
 * Mechanism-free on purpose. This says a file needs a moment and never what is
 * being done about it, so the panel's sentence stays true if a handler that is
 * not iCloud ever answers.
 */
export async function onAttachmentReadying(
  handler: (file: { id: string; name: string; readying: boolean }) => void,
): Promise<() => void> {
  if (!inTauri) return () => {}
  const { listen } = await import("@tauri-apps/api/event")
  return listen<{ id: string; name: string; readying: boolean }>(
    HOST_EVENTS.attachmentReadying,
    ({ payload }) => handler(payload),
  )
}

/**
 * Be told while a drag is over the panel, and nothing else.
 *
 * The page receives no drag events of its own any more, so without this the
 * drop target could not be drawn at all: the panel would take a dropped file
 * perfectly well while giving no sign beforehand that it would.
 *
 * Naming the drop rode along here for a while. It made one event two facts
 * with two audiences, and left the picker — which never drags — no way to say
 * the same thing, so every `+` selection went on guessing its draft. That is
 * {@link onAttachmentBatch} now, for both gestures.
 */
export async function onAttachmentDragging(
  handler: (dragging: boolean) => void,
): Promise<() => void> {
  if (!inTauri) return () => {}
  const { listen } = await import("@tauri-apps/api/event")
  return listen<{ dragging: boolean }>(HOST_EVENTS.attachmentDragging, ({ payload }) =>
    handler(payload.dragging),
  )
}

/**
 * Be told that an attach has begun, what it is called, and which gesture it was.
 *
 * Said before the host has looked at anything. It is the page's only chance to
 * bind that attach to the conversation it belongs to: the files themselves can
 * be three quarters of a minute later, by which time the open tab may be a
 * different one, and binding then put the tile and the attach over a draft the
 * files were never going to join.
 *
 * Both gestures say it, because both can be slow. Only the drop did once, and
 * a `+` selection of two placeholders therefore put its second tile on
 * whichever tab was open when the second file finished — blocking that draft's
 * send for a file that had gone somewhere else entirely.
 *
 * `gesture` is here because the two know their draft at different moments, and
 * this is the only part of that the host knows and the page does not. A drop
 * *is* the moment: the host says so as it lands, so the open tab is the tab it
 * landed on. A `"picked"` selection was begun by the page, which captured the
 * draft when `+` was pressed and has held it ever since; this arrives when the
 * picker closes, and the page answers with what it already knows rather than
 * reading the tab a second time. Two reads of the same fact agree only while
 * nothing can change between them, and that was resting on the picker being
 * modal — which is rfd's presentation choice, not ours.
 */
export async function onAttachmentBatch(
  handler: (batch: string, gesture: "picked" | "dropped") => void,
): Promise<() => void> {
  if (!inTauri) return () => {}
  const { listen } = await import("@tauri-apps/api/event")
  return listen<{ batch: string; gesture: "picked" | "dropped" }>(
    HOST_EVENTS.attachmentBatch,
    ({ payload }) => handler(payload.batch, payload.gesture),
  )
}

/**
 * Read the bytes of a file the picker already handed back.
 *
 * What happens to an attached file is decided by its type, not by how it was
 * attached: an image is uploaded and normalised wherever it came from, so one
 * chosen through the host picker still needs its bytes, and the filesystem is
 * the host's to read.
 *
 * **Takes the `ticket` from {@link ChosenFile}, never a path.** The host holds
 * the paths it minted tickets for and resolves the ticket itself, so this page
 * cannot name a file the host did not already agree to remember. A ticket is
 * good exactly once: presenting it again rejects with `ticket-already-used`,
 * one the host has no record of rejects with `ticket-unknown`, and one left
 * unread for ten minutes rejects with `ticket-expired`. All three are refusals
 * about the ticket and say nothing about any file — picking the file again is
 * the way out of the last two.
 *
 * `null` outside Tauri, where there is nothing to read a file with — the caller
 * there already has the bytes from the browser's own file input.
 *
 * This is deliberately a second read of that file, taken however long after the
 * choice the person spent composing. It may have been moved, replaced,
 * truncated or deleted in between, so the rejection is a typed
 * `attachments::FileNotAttached` — `file-unreadable` for a file that will not
 * read, `file-too-large` for one past what the panel will hold,
 * `not-a-regular-file` for a path that has become a directory or a pipe, and
 * `filesystem-stalled` for a disk that stopped answering — rather than an
 * assumption that a file once chosen still opens.
 *
 * An `ArrayBuffer` rather than an array of numbers: the host answers with
 * `tauri::ipc::Response`, which crosses as binary.
 */
export async function readAttachmentBytes(ticket: string): Promise<ArrayBuffer | null> {
  if (!inTauri) return null
  const { invoke } = await import("@tauri-apps/api/core")
  return invoke<ArrayBuffer>("read_attachment_bytes", { ticket })
}

/** Why a clicked link did nothing. Matches `host::NotOpened`. */
export type NotOpenedReason = "refused" | "opener-failed"

/** Which link did nothing, and why. Matches `host::LinkNotOpened`. */
export interface LinkNotOpened {
  url: string
  reason: NotOpenedReason
  /** The opener's own error, for the diagnostics rather than the screen. */
  detail: string | null
}

/**
 * Subscribes to a clicked link that did not open.
 *
 * Links leave the app rather than navigating this window, which has no address
 * bar to get back from. When one goes nowhere — a scheme the host refuses, or a
 * browser it could not start — the click is otherwise indistinguishable from a
 * dead page, so the host says so and the panel puts it on screen.
 */
export async function onLinkNotOpened(handler: (link: LinkNotOpened) => void) {
  if (!inTauri) return () => undefined
  const { listen } = await import("@tauri-apps/api/event")
  return listen<LinkNotOpened>(HOST_EVENTS.linkNotOpened, ({ payload }) =>
    handler(payload),
  )
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
  /**
   * The panel is up and setup is over, but this machine did not write it down.
   * The host deliberately leaves this window open for it: the write carries the
   * completion flag and the chosen agent in one update, so losing it loses the
   * choice too, and every conversation of this launch would run on the
   * gateway's default while the screen said the handoff worked.
   */
  | { outcome: "setup-not-recorded"; cause: unknown }
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
  // Not survivable in the way this used to claim. The host writes the
  // completion and the agent in one update, so a failure loses both:
  // `chosen_agent` then truthfully says nobody chose, the panel rightly
  // declines to remember that, and every conversation of this launch runs on
  // the gateway's default — the same failure the in-place handover exists to
  // prevent, reached by a different road. The host leaves this window up for
  // it, and the surface offers the write again.
  if (handoff.recordError) {
    return { outcome: "setup-not-recorded", cause: handoff.recordError }
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
 * Write setup off again, after a handoff whose write the host refused.
 *
 * The panel is already up, so this asks for the write and the close alone.
 * Asking for the whole handoff again would summon a panel that is on screen,
 * which re-anchors and refits a window somebody may have moved to.
 *
 * The agent travels again because nothing was recorded for the host to read it
 * back from, and this window still has the choice setup finished on.
 */
export async function retrySetupRecord(agent?: string): Promise<SetupHandoff> {
  if (!inTauri) return { outcome: "no-native-host" }
  const { invoke } = await import("@tauri-apps/api/core")
  let handoff: NativeSetupHandoff
  try {
    handoff = await invoke<NativeSetupHandoff>("retry_setup_record", {
      agent: agent ?? null,
    })
  } catch (cause) {
    // The host could not be asked at all. Nothing was written, and the screen
    // that asked is still the right one: it offers this again.
    return { outcome: "setup-not-recorded", cause }
  }
  if (handoff.recordError) {
    return { outcome: "setup-not-recorded", cause: handoff.recordError }
  }
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
 * `onboarding/ui/setup-gate.tsx`, whose effect asks directly rather than waiting.
 */
export async function revealSetupWindow() {
  if (!inTauri) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("reveal_setup_window")
}

/**
 * What the host has to say about the agent first-run setup chose.
 *
 * `"unavailable"` is kept apart from `"none"` on purpose. Both leave this
 * conversation on the gateway's own default, but only one of them is an answer:
 * a host that could not be asked may answer perfectly well a moment later, and
 * treating that as "nobody chose" is how a saved choice gets dropped for good.
 */
export type ChosenAgent =
  /** Setup recorded this agent, and every conversation should run on it. */
  | { outcome: "chosen"; agent: string }
  /** Nobody has chosen: a first run still in progress, a setup that was left,
   *  or a browser with no host to ask. */
  | { outcome: "none" }
  /** The host could not be asked. Not an answer, and not one to remember. */
  | { outcome: "unavailable" }

/**
 * The agent first-run setup chose, as the host recorded it.
 *
 * Read rather than remembered: setup runs in its own window, which is gone by
 * the time the panel needs the answer.
 *
 * Never rejects. A host that cannot be read is survivable — the conversation
 * starts on the gateway's default, which is a worse answer and not a broken
 * panel — but it is reported as the failure it is, because a caller that
 * remembers answers must not remember this one.
 */
export async function loadChosenAgent(): Promise<ChosenAgent> {
  if (!inTauri) return { outcome: "none" }
  try {
    // The import is inside the try with the call it makes. Loading the host
    // module is a fetch like any other and can fail on its own; left outside,
    // that failure would come back as a rejection from a function whose whole
    // contract is that it answers.
    const { invoke } = await import("@tauri-apps/api/core")
    const agent = await invoke<string | null>("chosen_agent")
    return agent === null || agent === undefined
      ? { outcome: "none" }
      : { outcome: "chosen", agent }
  } catch (cause) {
    console.warn("[nessa] could not read the agent setup chose", cause)
    return { outcome: "unavailable" }
  }
}

/** Whether this page runs inside the trusted desktop host. */
export function hasNativeHost(): boolean {
  return inTauri
}

/** Load only the bundled chat credential from native private storage. */
export async function loadAssignedSurfaceCredential(
  stage: string,
  url: string,
): Promise<string> {
  if (!inTauri) throw new Error("A native host is required for local credential storage")
  const { invoke } = await import("@tauri-apps/api/core")
  try {
    return await invoke<string>("load_surface_credential", { stage, url })
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

/** Resolve the private endpoint publication through the native host. */
export async function loadAssignedGatewayEndpoint(
  stage: string,
): Promise<string | undefined> {
  if (!inTauri) return undefined
  const { invoke } = await import("@tauri-apps/api/core")
  try {
    return (await invoke<string | null>("load_gateway_endpoint", { stage })) ?? undefined
  } catch (error) {
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
        : "Could not verify the desktop gateway endpoint.",
      { cause: error },
    )
  }
}
