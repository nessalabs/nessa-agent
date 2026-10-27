import { memo, useCallback } from "react"
import { SplitPanes, type ShownPane, type SplitPanesSource } from "../../../split-panes"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectFailure,
  selectFailureStages,
  selectPaneSession,
  selectWindowWidget,
} from "../../adapters/store/selectors"
import { paneItemOf } from "../../model/pane-item"
import { EmptyWorkspace } from "./empty-state"
import { Pane } from "./pane"
import { WidgetWindow } from "./widget-window"
import "./panes.css"

/**
 * The chat area: the window's panes as split panes (`SplitPanes`), each a
 * session's `Pane` on the frame the grid gives it, over the
 * workspace's source — the same one the window's drag reads
 * (`workspaceSplitPanes`). With no pane to show, the workspace's empty
 * state, which says why. Over them, while it shows one, the window's widget
 * (`WidgetWindow`).
 */
export const PaneGrid = memo(function PaneGrid({ source }: { source: SplitPanesSource }) {
  const failure = useWorkspaceSelector(selectFailure)
  const stages = useWorkspaceSelector(selectFailureStages)
  const windowWidget = useWorkspaceSelector(selectWindowWidget)
  const renderPane = useCallback((shown: ShownPane) => <ShownPaneOf {...shown} />, [])
  return (
    <main
      className="workspace-chat"
      aria-label="Conversations"
      data-window-widget={windowWidget ? "" : undefined}
    >
      <SplitPanes
        source={source}
        renderPane={renderPane}
        empty={<EmptyWorkspace failure={failure} stages={stages} />}
      />
      <WidgetWindow />
    </main>
  )
})

/**
 * A session's pane. The grid keeps a closed pane's box for three frames; the
 * layout no longer has the key then, so the box keeps the session it was
 * placed with.
 */
const ShownPaneOf = memo(function ShownPaneOf({ placement, frame, multi }: ShownPane) {
  const sessionId = useWorkspaceSelector((state) =>
    selectPaneSession(state, placement.key),
  )
  const held = sessionId === null ? paneItemOf(placement.item) : null
  return (
    <Pane
      placement={placement}
      frame={frame}
      multi={multi}
      heldSession={held?.kind === "session" ? held.sessionId : undefined}
    />
  )
})
