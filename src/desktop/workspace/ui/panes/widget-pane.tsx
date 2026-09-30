import { memo } from "react"
import {
  WidgetInPane,
  useWidgetSession,
  useWidgetTitle,
  useWidgetTrail,
  widgetKind,
} from "../../../widgets"
import { closePane, focusPane, openBeside } from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import { selectFocusedPaneKey, selectSession } from "../../adapters/store/selectors"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { widgetItem, type WidgetItem } from "../../model/pane-item"
import type { PaneFrame } from "../../../split-panes"
import type { PaneKey } from "../../../split-panes/model/pane-layout"
import { AgentTile } from "../chrome/agent-tile"
import { IconButton } from "../chrome/icon-button"
import { usePictureInConversationsPreference } from "../../../adapters/window-preferences"
import { HeaderSliver } from "../../../ui/header-art"
import { tooltip } from "../../../ui/tooltip"
import { useWorkspaceFrame } from "../workspace-frame"

/**
 * A pane that shows a widget — an experiment — rather than a conversation:
 * the same glass, header and place in the grid as a chat, so it splits,
 * resizes, moves and closes like one. Its header links back to the
 * conversation it belongs to; the widget draws everything else.
 */
export const WidgetPane = memo(function WidgetPane({
  pane,
  widget,
  frame,
  multi,
}: {
  pane: PaneKey
  widget: WidgetItem
  frame: PaneFrame
  multi: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
  const workspace = useWorkspaceFrame()
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === pane)
  const title = useWidgetTitle(widget) ?? "Widget"
  const [picture] = usePictureInConversationsPreference()
  const originId = useWidgetSession(widget)
  const trail = useWidgetTrail(widget)
  const origin = useWorkspaceSelector((state) =>
    originId === undefined ? undefined : selectSession(state, originId),
  )
  return (
    <article
      className="workspace-pane"
      data-widget-pane
      {...frame}
      data-focused={(focused && multi) || undefined}
      {...{ [focusedPaneAttribute]: focused || undefined }}
      aria-label={title}
      onPointerDown={() => {
        if (!focused) dispatch(focusPane({ pane }))
      }}
      onFocusCapture={() => {
        if (selectFocusedPaneKey(store.getState()) !== pane) dispatch(focusPane({ pane }))
      }}
    >
      {/* The same sliver of the header picture a conversation's pane wears. */}
      {picture === "on" ? <HeaderSliver moving={focused} /> : null}
      <header
        className="workspace-pane-header"
        data-split-keeps="top-left"
        data-tauri-drag-region={multi ? undefined : true}
        data-drag-pane={multi ? pane : undefined}
      >
        <div className="workspace-pane-name" data-shown>
          {origin ? (
            <button
              type="button"
              className="workspace-widget-origin"
              draggable={false}
              {...tooltip("Open the conversation it belongs to")}
              onClick={(event) => {
                event.stopPropagation()
                // Focused where it is, or opened beside this pane.
                dispatch(openBeside({ sessionId: origin.id, target: pane, side: "left" }))
              }}
            >
              <AgentTile model={origin.model} size={16} />
              <span className="workspace-truncate">{origin.title}</span>
            </button>
          ) : null}
          {origin ? (
            <span className="workspace-widget-crumb" aria-hidden>
              ›
            </span>
          ) : null}
          {trail ? (
            <>
              <button
                type="button"
                className="workspace-widget-origin workspace-widget-up"
                draggable={false}
                onClick={(event) => {
                  event.stopPropagation()
                  trail.onBack()
                }}
              >
                {widgetKind(widget)}
              </button>
              <span className="workspace-widget-crumb" aria-hidden>
                ›
              </span>
              <span className="workspace-pane-kind workspace-truncate">
                {trail.label}
              </span>
            </>
          ) : (
            <span className="workspace-pane-kind">{widgetKind(widget)}</span>
          )}
        </div>
        <span
          className="workspace-spacer"
          data-tauri-drag-region={multi ? undefined : true}
        />
        <div className="workspace-pane-actions">
          <IconButton
            icon="close"
            label="Close Pane"
            shortcut={workspace.shortcut("closePane")}
            draggable={false}
            onClick={(event) => {
              event.stopPropagation()
              dispatch(closePane({ pane }))
            }}
          />
        </div>
      </header>
      <div className="workspace-pane-body" data-split-through>
        <WidgetInPane
          widget={widget}
          onOpenWidget={(other) =>
            void dispatch(openBeside({ sessionId: widgetItem(other), target: pane }))
          }
        />
      </div>
    </article>
  )
})
