/**
 * The two keys that leave setup, and the rule about when they are setup's.
 *
 * Escape is what Escape means on a surface like this, and Command-Q closes
 * *this window* rather than quitting Nessa — setup is a window in front of an
 * app that is still running.
 *
 * Both are only setup's while setup is on screen. The browser path keeps the
 * same component mounted after setup finishes — it starts rendering the panel
 * in place of the setup UI — so a listener that stayed attached went on eating
 * every Escape and every Command-Q the person pressed while using the app, and
 * neither the panel nor the browser ever saw them. Nothing is attached at all
 * when setup is not showing.
 */

/** The parts of a key press this rule reads. */
export interface DismissKeyPress {
  key: string
  repeat: boolean
  metaKey: boolean
  ctrlKey: boolean
  preventDefault(): void
}

/** Where the listener goes. `window` in the app; a stub in a test. */
export interface DismissKeyTarget {
  addEventListener(type: "keydown", handler: (event: DismissKeyPress) => void): void
  removeEventListener(type: "keydown", handler: (event: DismissKeyPress) => void): void
}

/** Whether this press is one of the two ways out. */
function dismissesSetup(event: DismissKeyPress): boolean {
  if (event.repeat) return false
  if (event.key === "Escape") return true
  return event.key.toLowerCase() === "q" && (event.metaKey || event.ctrlKey)
}

/**
 * Listen for the ways out while setup is showing, and return the way to stop.
 *
 * While `active` is false nothing is listening, so nothing is intercepted: the
 * returned stop is then a no-op with nothing to undo.
 */
export function listenForDismiss(
  target: DismissKeyTarget,
  active: boolean,
  dismiss: () => void,
): () => void {
  if (!active) return () => undefined
  function onKeyDown(event: DismissKeyPress) {
    if (!dismissesSetup(event)) return
    event.preventDefault()
    dismiss()
  }
  target.addEventListener("keydown", onKeyDown)
  return () => target.removeEventListener("keydown", onKeyDown)
}
