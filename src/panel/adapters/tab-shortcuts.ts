import { useEffect } from "react"

import { host } from "../../host"

export type TabShortcutAction = "open" | "close"

type ChordSurface = "desktop" | "browser"

type KeyChord = {
  key: string
  metaKey: boolean
  ctrlKey: boolean
  altKey: boolean
  shiftKey: boolean
  repeat: boolean
}

/**
 * Map a key event to a tab action for the given surface.
 *
 * Desktop (native panel): Cmd/Ctrl+T or N opens; Cmd/Ctrl+W closes.
 * Browser preview: Cmd/Ctrl+Shift+T / W so we do not fight the browser's own
 * tab and window chords.
 *
 * Returns null when the chord is not ours — including key-repeat, so holding
 * T does not open a stack of tabs.
 */
export function tabShortcutAction(
  event: KeyChord,
  surface: ChordSurface,
): TabShortcutAction | null {
  if (event.repeat) return null
  if (event.altKey) return null
  if (!(event.metaKey || event.ctrlKey)) return null

  const key = event.key.length === 1 ? event.key.toLowerCase() : event.key

  if (surface === "browser") {
    if (!event.shiftKey) return null
    if (key === "t") return "open"
    if (key === "w") return "close"
    return null
  }

  if (event.shiftKey) return null
  if (key === "t" || key === "n") return "open"
  if (key === "w") return "close"
  return null
}

function chordSurface(): ChordSurface {
  return host.kind === "browser" ? "browser" : "desktop"
}

/**
 * In-panel tab chords. Listens on the page only — never a global OS shortcut —
 * so they fire when this surface has focus and stay out of the way of other
 * Nessa surfaces later.
 */
export function useTabShortcuts(actions: {
  openTab: () => void
  closeActiveTab: () => void
}) {
  const { openTab, closeActiveTab } = actions

  useEffect(() => {
    const surface = chordSurface()

    function onKeyDown(event: KeyboardEvent) {
      if (!document.hasFocus()) return

      const action = tabShortcutAction(event, surface)
      if (!action) return

      event.preventDefault()
      if (action === "open") openTab()
      else closeActiveTab()
    }

    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [openTab, closeActiveTab])
}
