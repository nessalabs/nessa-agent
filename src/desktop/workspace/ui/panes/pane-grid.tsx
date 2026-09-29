import { memo, useCallback } from "react"
import { SplitPanes, type ShownPane, type SplitPanesSource } from "../../../split-panes"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFailure } from "../../adapters/store/selectors"
import { EmptyWorkspace } from "./empty-state"
import { Pane } from "./pane"
import "./panes.css"

/**
 * The chat area: the window's conversations as split panes (`SplitPanes`),
 * each a `Pane` on the frame the grid gives it, over the workspace's source
 * — the same one the window's drag reads (`workspaceSplitPanes`). With no
 * pane to show, the workspace's empty state, which says why.
 */
export const PaneGrid = memo(function PaneGrid({ source }: { source: SplitPanesSource }) {
  const failure = useWorkspaceSelector(selectFailure)
  const renderPane = useCallback(
    ({ placement, frame, multi }: ShownPane) => (
      <Pane placement={placement} frame={frame} multi={multi} />
    ),
    [],
  )
  return (
    <main className="workspace-chat" aria-label="Conversations">
      <SplitPanes
        source={source}
        renderPane={renderPane}
        empty={<EmptyWorkspace failure={failure} />}
      />
    </main>
  )
})
