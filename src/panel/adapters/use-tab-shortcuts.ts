import { useEffect, useState } from "react"

import type { ShortcutsDocument } from "@nessa/client"

import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { applyShortcuts, loadShortcuts } from "../../host/window"
import { useSession } from "../../session"
import {
  chordSurface,
  matchFocusedShortcut,
  type FocusedPanelAction,
} from "./tab-shortcuts"

const bundledDefaults = defaults as ShortcutsDocument

/**
 * Hydrate shortcuts from the host cache (or bundled defaults in the browser),
 * refresh from HelloOk, and dispatch focused tab actions — never literal chords.
 */
export function useTabShortcuts(actions: {
  openTab: () => void
  closeActiveTab: () => void
  activateTab: (target: {
    index?: number
    conversationId?: string
  }) => void
}) {
  const { openTab, closeActiveTab, activateTab } = actions
  const session = useSession()
  const [shortcuts, setShortcuts] = useState<ShortcutsDocument>(bundledDefaults)

  useEffect(() => {
    let cancelled = false
    void loadShortcuts().then((loaded) => {
      if (!cancelled && loaded) setShortcuts(loaded)
    })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    const fromHello = session.hello?.shortcuts
    if (!fromHello) return
    setShortcuts(fromHello)
    void applyShortcuts(fromHello)
  }, [session.hello])

  useEffect(() => {
    const surface = chordSurface()

    function onKeyDown(event: KeyboardEvent) {
      if (!globalThis.document.hasFocus()) return

      const matched = matchFocusedShortcut(event, shortcuts, surface)
      if (!matched) return

      event.preventDefault()
      dispatchFocused(matched, { openTab, closeActiveTab, activateTab })
    }

    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [shortcuts, openTab, closeActiveTab, activateTab])
}

function dispatchFocused(
  matched: FocusedPanelAction,
  actions: {
    openTab: () => void
    closeActiveTab: () => void
    activateTab: (target: {
      index?: number
      conversationId?: string
    }) => void
  },
) {
  if (matched.action === "panel.newTab") actions.openTab()
  else if (matched.action === "panel.closeTab") actions.closeActiveTab()
  else
    actions.activateTab({
      index: matched.index,
      conversationId: matched.conversationId,
    })
}
