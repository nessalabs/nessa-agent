import { useEffect, useLayoutEffect, useState } from "react"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { HeaderSliver } from "../../../ui/header-art"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { useWorkspaceStore } from "../../adapters/store/hooks"
import { selectFocusedPaneKey } from "../../adapters/store/selectors"

/** Puts the focused mark on the pane's article. The pane itself does not render. */
function markPane(paneKey: PaneKey, multi: boolean, focused: boolean) {
  const article = document.querySelector<HTMLElement>(`[data-pane-key="${paneKey}"]`)
  if (!article) return
  if (focused && multi) article.setAttribute("data-focused", "")
  else article.removeAttribute("data-focused")
  if (focused) article.setAttribute(focusedPaneAttribute, "")
  else article.removeAttribute(focusedPaneAttribute)
}

/**
 * The focused mark, without rendering the pane. The click that moves focus
 * writes the mark here, before paint, so the caret does not stay in the pane
 * it left and the conversation does not render for the click.
 */
export function PaneFocusMark({ paneKey, multi }: { paneKey: PaneKey; multi: boolean }) {
  const store = useWorkspaceStore()
  useLayoutEffect(() => {
    let frame = 0
    let focused = selectFocusedPaneKey(store.getState()) === paneKey
    // Before this pane's first paint, so the caret finds the mark on that
    // frame. A later change waits for the next frame: writing it in the
    // click restyles the pane it leaves.
    markPane(paneKey, multi, focused)
    const unsubscribe = store.subscribe(() => {
      const next = selectFocusedPaneKey(store.getState()) === paneKey
      if (next === focused) return
      focused = next
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => markPane(paneKey, multi, next))
    })
    return () => {
      cancelAnimationFrame(frame)
      unsubscribe()
    }
  }, [store, paneKey, multi])
  return null
}

/** The picture follows focus on the next frame. The click does not render it. */
export function FocusedSliver({ paneKey }: { paneKey: PaneKey }) {
  const store = useWorkspaceStore()
  const [moving, setMoving] = useState(
    () => selectFocusedPaneKey(store.getState()) === paneKey,
  )
  useEffect(() => {
    let frame = 0
    let movingNow = selectFocusedPaneKey(store.getState()) === paneKey
    const unsubscribe = store.subscribe(() => {
      const next = selectFocusedPaneKey(store.getState()) === paneKey
      if (next === movingNow) return
      movingNow = next
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() => setMoving(next))
    })
    return () => {
      cancelAnimationFrame(frame)
      unsubscribe()
    }
  }, [store, paneKey])
  return <HeaderSliver moving={moving} />
}
