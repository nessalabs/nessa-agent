import { useEffect, useState } from "react"
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
  useEffect(() => {
    let focused = selectFocusedPaneKey(store.getState()) === paneKey
    markPane(paneKey, multi, focused)
    return store.subscribe(() => {
      const next = selectFocusedPaneKey(store.getState()) === paneKey
      if (next === focused) return
      focused = next
      markPane(paneKey, multi, focused)
    })
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
    const unsubscribe = store.subscribe(() => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(() =>
        setMoving(selectFocusedPaneKey(store.getState()) === paneKey),
      )
    })
    return () => {
      cancelAnimationFrame(frame)
      unsubscribe()
    }
  }, [store, paneKey])
  return <HeaderSliver moving={moving} />
}
