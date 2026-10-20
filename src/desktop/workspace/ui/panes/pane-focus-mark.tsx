import { useLayoutEffect } from "react"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { HeaderSliver } from "../../../ui/header-art"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFocusedPaneKey } from "../../adapters/store/selectors"

/**
 * The focused mark, without rendering the pane. The click that moves focus
 * would otherwise reconcile the conversation it left. The mark is on the
 * article before paint, so the caret does not stay in that pane.
 */
export function PaneFocusMark({ paneKey, multi }: { paneKey: PaneKey; multi: boolean }) {
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === paneKey)
  useLayoutEffect(() => {
    const article = document.querySelector<HTMLElement>(`[data-pane-key="${paneKey}"]`)
    if (!article) return
    if (focused && multi) article.setAttribute("data-focused", "")
    else article.removeAttribute("data-focused")
    if (focused) article.setAttribute(focusedPaneAttribute, "")
    else article.removeAttribute(focusedPaneAttribute)
  }, [focused, paneKey, multi])
  return null
}

/** The picture follows focus. The conversation under it does not render for that. */
export function FocusedSliver({ paneKey }: { paneKey: PaneKey }) {
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === paneKey)
  return <HeaderSliver moving={focused} />
}
