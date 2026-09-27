import { useEffect, useState } from "react"

import type { ShortcutsDocument } from "@nessa/client"

import defaults from "../../../protocol/defaults/shortcuts.v1.json"
import { loadShortcuts } from "../../host/window"
import {
  chordSurface,
  matchFocusedShortcut,
  type FocusedPanelAction,
} from "./tab-shortcuts"

const bundledDefaults = defaults as ShortcutsDocument

/**
 * Hydrate shortcuts from the host cache (or bundled defaults in the browser),
 * and dispatch focused tab actions.
 */
export function useTabShortcuts(actions: {
  openTab: () => void
  closeActiveTab: () => void
  moveActiveTab: (direction: -1 | 1) => void
  activateTab: (target: { index?: number; conversationId?: string }) => void
}) {
  const { openTab, closeActiveTab, moveActiveTab, activateTab } = actions
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
    const surface = chordSurface()

    function onKeyDown(event: KeyboardEvent) {
      if (!globalThis.document.hasFocus()) return

      const matched = matchFocusedShortcut(event, shortcuts, surface)
      if (!matched) return

      event.preventDefault()
      dispatchFocused(matched, { openTab, closeActiveTab, moveActiveTab, activateTab })
    }

    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [shortcuts, openTab, closeActiveTab, moveActiveTab, activateTab])
}

function dispatchFocused(
  matched: FocusedPanelAction,
  actions: {
    openTab: () => void
    closeActiveTab: () => void
    moveActiveTab: (direction: -1 | 1) => void
    activateTab: (target: { index?: number; conversationId?: string }) => void
  },
) {
  if (matched.action === "panel.newTab") actions.openTab()
  else if (matched.action === "panel.closeTab") actions.closeActiveTab()
  else if (matched.action === "panel.previousTab") actions.moveActiveTab(-1)
  else if (matched.action === "panel.nextTab") actions.moveActiveTab(1)
  else if (matched.action === "panel.activateTab")
    actions.activateTab({
      index: matched.index,
      conversationId: matched.conversationId,
    })
}
