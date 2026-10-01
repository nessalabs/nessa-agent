import { memo, useCallback } from "react"
import { shallowEqual } from "react-redux"
import { SplitPanes, type ShownPane, type SplitPanesSource } from "../../../split-panes"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFailure, selectPaneWidget } from "../../adapters/store/selectors"
import { EmptyWorkspace } from "./empty-state"
import { Pane } from "./pane"
import { WidgetPane } from "./widget-pane"
import { WidgetWindow } from "./widget-window"
import "./panes.css"

/**
 * The chat area: the window's panes as split panes (`SplitPanes`), each a
 * session's `Pane` or a `WidgetPane` on the frame the grid gives it, over the
 * workspace's source — the same one the window's drag reads
 * (`workspaceSplitPanes`). With no pane to show, the workspace's empty
 * state, which says why. Over them, while it shows one, the window's widget
 * (`WidgetWindow`).
 */
export const PaneGrid = memo(function PaneGrid({ source }: { source: SplitPanesSource }) {
  const failure = useWorkspaceSelector(selectFailure)
  const renderPane = useCallback((shown: ShownPane) => <ShownPaneOf {...shown} />, [])
  return (
    <main className="workspace-chat" aria-label="Conversations">
      <SplitPanes
        source={source}
        renderPane={renderPane}
        empty={<EmptyWorkspace failure={failure} />}
      />
      <WidgetWindow />
    </main>
  )
})

/** A pane, by what it shows: a session, or a widget. */
const ShownPaneOf = memo(function ShownPaneOf({ placement, frame, multi }: ShownPane) {
  const widget = useWorkspaceSelector(
    (state) => selectPaneWidget(state, placement.key),
    shallowEqual,
  )
  return widget ? (
    <WidgetPane pane={placement.key} widget={widget} frame={frame} multi={multi} />
  ) : (
    <Pane placement={placement} frame={frame} multi={multi} />
  )
})
