import * as React from "react"

import { onFocusComposer, onToggleSurface, setFrosted } from "../../host"
import { type Surface } from "../model"
import { usePanelFrame } from "./panel-frame"

export function useHostPanel(
  surface: Surface,
  toggleSurface: () => void,
  composer: React.RefObject<HTMLTextAreaElement | null>,
) {
  usePanelFrame()

  React.useEffect(() => {
    let stale = false
    const subscription = onFocusComposer(() => {
      if (!stale) composer.current?.focus()
    })
    return () => {
      stale = true
      void subscription.then((unlisten) => unlisten())
    }
  }, [composer])

  React.useEffect(() => {
    void setFrosted(surface === "translucent")
  }, [surface])

  React.useEffect(() => {
    let stale = false
    const subscription = onToggleSurface(() => {
      if (!stale) toggleSurface()
    })
    return () => {
      stale = true
      void subscription.then((unlisten) => unlisten())
    }
  }, [toggleSurface])
}
