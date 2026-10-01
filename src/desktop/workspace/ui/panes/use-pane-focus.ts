import { useMemo } from "react"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { focusPane } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceStore } from "../../adapters/store/hooks"
import { selectFocusedPaneKey } from "../../adapters/store/selectors"

/**
 * A pane taking focus from the page — a press, or the caret Tabbing in — for
 * whichever pane it is, a session's or a widget's. The other direction, the
 * caret following the store, is `adapters/dom/focus.ts`.
 */
export function usePaneFocus(pane: PaneKey) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  return useMemo(() => {
    const take = () => {
      if (selectFocusedPaneKey(store.getState()) !== pane) dispatch(focusPane({ pane }))
    }
    return { onPointerDown: take, onFocusCapture: take }
  }, [dispatch, store, pane])
}
