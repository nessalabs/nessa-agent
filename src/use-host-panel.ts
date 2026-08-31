import * as React from "react"

import {
  onFocusComposer,
  onToggleSurface,
  setFrosted,
} from "./host-window"
import { usePanelFrame } from "./use-panel-frame"

/**
 * Wires the panel to the desktop host: frost, the tray's surface request,
 * and handing the caret to the composer when the panel is summoned.
 *
 * Lives in a hook so `App` can render without owning the seam.
 */
export function useHostPanel(
  surface: "translucent" | "clear",
  toggleSurface: () => void,
  composer: React.RefObject<HTMLTextAreaElement | null>,
) {
  usePanelFrame()

  React.useEffect(() => {
    const subscription = onFocusComposer(() => composer.current?.focus())
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [composer])

  React.useEffect(() => {
    void setFrosted(surface === "translucent")
  }, [surface])

  React.useEffect(() => {
    const subscription = onToggleSurface(toggleSurface)
    return () => {
      void subscription.then((unlisten) => unlisten())
    }
  }, [toggleSurface])
}
